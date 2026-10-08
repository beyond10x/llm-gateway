//! The gateway's counters and its per-call usage record (rows R4, O1, O2; story:gateway-
//! observability). The specification is `spec/domains/telemetry.yaml`.
//!
//! The counters are llmgw's 13 series, names and labels verbatim, rendered here as Prometheus
//! text by hand: the crate takes no dependency for it. A record leaves the crate only through
//! [`UsageRecords`], which the embedding implements, as it implements the target source.

use crate::{error::RefusalCode, relay::Wire};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// How one call ended, from the gateway's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// The target's answer reached the client, whatever its status (a 4xx or 500 included).
    Relayed,
    /// The gateway refused it, `target-unavailable` and `model-cold-start` included.
    Refused,
    /// The target could not be reached or failed: `upstream-failed` (502).
    UpstreamFailed,
}

/// What the gateway learned about one model call it was asked to relay
/// (`llm-gateway.telemetry.UsageRecord`). One is made for every request to a wire path that
/// passed owner authentication, once its answer or refusal is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// The relayed model's alias the body named. `None` when it named no relayed model.
    pub model: Option<String>,
    pub wire: Wire,
    pub disposition: Disposition,
    /// Present exactly when `disposition` is not [`Disposition::Relayed`].
    pub refusal: Option<RefusalCode>,
    /// The status line the client received.
    pub status: u16,
    /// The status the target answered with: the relayed one, or the 502, 503 or 504 the
    /// gateway answered `upstream-failed` instead. `None` when no target answered: a refusal
    /// before any target was asked, or a target that could not be reached or closed without
    /// an answer.
    pub target_status: Option<u16>,
    /// The model name the target reported. Not read yet (story:usage-records): always `None`.
    pub reported_model: Option<String>,
    /// Token counters the target reported. Not read yet (story:usage-records): always `None`.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    /// Decoded body bytes of the target's answer relayed to the client; 0 for a refusal.
    pub response_bytes: u64,
    /// From the end of the request head to the last byte relayed or the refusal written.
    pub duration_ms: u64,
}

/// Where the gateway hands each [`UsageRecord`]. The embedding implements it; it is called on
/// the connection's own thread before the connection is closed, so it should return quickly.
pub trait UsageRecords: Send + Sync {
    fn record(&self, record: &UsageRecord);
}

/// Hands records to nobody: what a relay composed without [`crate::Relay::with_records`] uses.
pub(crate) struct NoRecords;

impl UsageRecords for NoRecords {
    fn record(&self, _record: &UsageRecord) {}
}

/// One process-wide series: its name and help text.
struct Series {
    name: &'static str,
    help: &'static str,
}

/// The eight process-wide series, in llmgw's order (matrix row O1).
const PROCESS: [Series; 8] = [
    Series {
        name: "llmgw_inference_requests_total",
        help: "Model calls the gateway was asked to relay, one per usage record.",
    },
    Series {
        name: "llmgw_upstream_failures_total",
        help: "Model calls whose target could not be reached or closed without an answer.",
    },
    Series {
        name: "llmgw_instruction_views_total",
        help: "Setup instruction pages served at GET /.",
    },
    Series {
        name: "llmgw_pod_starts_total",
        help: "Pod creates the pool submitted.",
    },
    Series {
        name: "llmgw_pod_start_failures_total",
        help: "Pods that could not be started or failed while starting.",
    },
    Series {
        name: "llmgw_pod_reaps_total",
        help: "Deployments and pods the cleanup pass stopped.",
    },
    Series {
        name: "llmgw_endpoint_invalidations_total",
        help: "Targets the gateway reported failed to its target source.",
    },
    Series {
        name: "llmgw_cold_start_wait_seconds_total",
        help: "Seconds requests spent held for a starting target.",
    },
];

/// The five per-model, per-wire series, in llmgw's order (matrix row O2).
const ROUTE: [Series; 5] = [
    Series {
        name: "llmgw_route_requests_total",
        help: "Model calls per model and wire.",
    },
    Series {
        name: "llmgw_route_refusals_total",
        help: "Model calls refused or left without a target answer, per model and wire.",
    },
    Series {
        name: "llmgw_route_upstream_status_failures_total",
        help: "Model calls whose target answered a status outside 2xx, per model and wire.",
    },
    Series {
        name: "llmgw_route_cold_start_holds_total",
        help: "Model calls held for a starting target, per model and wire.",
    },
    Series {
        name: "llmgw_route_response_bytes_total",
        help: "Body bytes of target answers relayed to clients, per model and wire.",
    },
];

/// A process-wide counter, by its place in [`PROCESS`].
#[derive(Clone, Copy)]
enum Process {
    InferenceRequests,
    UpstreamFailures,
    PodStarts = 3,
    PodStartFailures,
    PodReaps,
    EndpointInvalidations,
    /// Kept in milliseconds and rendered in seconds.
    ColdStartWait,
}

/// A per-route counter, by its place in [`ROUTE`].
#[derive(Clone, Copy)]
enum Route {
    Requests,
    Refusals,
    StatusFailures,
    ColdStartHolds,
    ResponseBytes,
}

/// The gateway's counters. A gateway owns one; an embedding that counts pod events itself
/// shares it with the relay through [`crate::Relay::with_metrics`]. Every counter only grows.
#[derive(Debug, Default)]
pub struct Metrics {
    process: [AtomicU64; 8],
    /// Per (alias, wire) a relayed model declares, registered at zero when the relay is
    /// composed. A call on any other pair feeds the process-wide series only.
    routes: Mutex<BTreeMap<(String, Wire), [u64; 5]>>,
}

impl Metrics {
    /// Counts a pod create the pool submitted (`llmgw_pod_starts_total`).
    pub fn count_pod_start(&self) {
        self.add(Process::PodStarts, 1);
    }

    /// Counts a pod that could not be started or failed while starting
    /// (`llmgw_pod_start_failures_total`).
    pub fn count_pod_start_failure(&self) {
        self.add(Process::PodStartFailures, 1);
    }

    /// Counts deployments and pods a cleanup pass stopped (`llmgw_pod_reaps_total`).
    pub fn count_pod_reaps(&self, stopped: u64) {
        self.add(Process::PodReaps, stopped);
    }

    /// The counters as Prometheus text (format 0.0.4): one `# HELP` and `# TYPE` pair and its
    /// samples per series, process-wide first, each route in alias and wire order.
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (index, series) in PROCESS.iter().enumerate() {
            header(&mut text, series);
            let value = self.process[index].load(Ordering::SeqCst);
            if index == Process::ColdStartWait as usize {
                let _ = writeln!(text, "{} {}", series.name, seconds(value));
            } else {
                let _ = writeln!(text, "{} {value}", series.name);
            }
        }
        let routes = self.routes();
        for (index, series) in ROUTE.iter().enumerate() {
            header(&mut text, series);
            for ((alias, wire), counters) in routes.iter() {
                let _ = writeln!(
                    text,
                    "{}{{model=\"{}\",wire=\"{}\"}} {}",
                    series.name,
                    escape(alias),
                    wire.label(),
                    counters[index]
                );
            }
        }
        text
    }

    /// Registers a (model, wire) pair at zero. Registering one twice changes nothing.
    pub(crate) fn register(&self, alias: &str, wire: Wire) {
        self.routes()
            .entry((alias.to_string(), wire))
            .or_insert([0; 5]);
    }

    /// Feeds the six series a record feeds, as llmgw (`src/lib.rs:598-629` at 048ebd8): a call
    /// no target answered is an upstream failure and a route refusal; a call whose target
    /// answered outside 2xx is a status failure, relayed or not.
    pub(crate) fn record(&self, record: &UsageRecord) {
        self.add(Process::InferenceRequests, 1);
        let unanswered =
            record.disposition == Disposition::UpstreamFailed && record.target_status.is_none();
        if unanswered {
            self.add(Process::UpstreamFailures, 1);
        }
        let Some(alias) = &record.model else {
            return;
        };
        let mut routes = self.routes();
        if let Some(counters) = routes.get_mut(&(alias.clone(), record.wire)) {
            counters[Route::Requests as usize] += 1;
            if record.disposition == Disposition::Refused || unanswered {
                counters[Route::Refusals as usize] += 1;
            }
            if record
                .target_status
                .is_some_and(|status| !(200..300).contains(&status))
            {
                counters[Route::StatusFailures as usize] += 1;
            }
            counters[Route::ResponseBytes as usize] =
                counters[Route::ResponseBytes as usize].saturating_add(record.response_bytes);
        }
    }

    /// Counts a target the gateway reported failed.
    pub(crate) fn count_invalidation(&self) {
        self.add(Process::EndpointInvalidations, 1);
    }

    /// Counts a request its target source held for a starting target, and how long it waited.
    pub(crate) fn count_cold_start_hold(&self, alias: &str, wire: Wire, waited: Duration) {
        self.add(
            Process::ColdStartWait,
            u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
        );
        if let Some(counters) = self.routes().get_mut(&(alias.to_string(), wire)) {
            counters[Route::ColdStartHolds as usize] += 1;
        }
    }

    fn add(&self, counter: Process, amount: u64) {
        let counter = &self.process[counter as usize];
        let _ = counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            Some(value.saturating_add(amount))
        });
    }

    fn routes(&self) -> std::sync::MutexGuard<'_, BTreeMap<(String, Wire), [u64; 5]>> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Milliseconds as decimal seconds without trailing zeros: `0`, `1.5`, `1.239`.
fn seconds(milliseconds: u64) -> String {
    let (whole, fraction) = (milliseconds / 1000, milliseconds % 1000);
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:03}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

fn header(text: &mut String, series: &Series) {
    let _ = writeln!(text, "# HELP {} {}", series.name, series.help);
    let _ = writeln!(text, "# TYPE {} counter", series.name);
}

/// A label value as the text format writes it: `\` and `"` escaped. An alias is printable
/// US-ASCII (`Label`), so it holds no line feed.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{Disposition, Metrics, UsageRecord};
    use crate::{error::RefusalCode, relay::Wire};
    use std::time::Duration;

    #[test]
    fn a_label_value_escapes_its_backslash_and_quote() {
        let metrics = Metrics::default();
        metrics.register("a\"b\\c", Wire::Chat);
        assert!(
            metrics
                .render()
                .contains("llmgw_route_requests_total{model=\"a\\\"b\\\\c\",wire=\"chat\"} 0\n")
        );
    }

    #[test]
    fn the_cold_start_wait_is_rendered_in_seconds() {
        let metrics = Metrics::default();
        metrics.count_cold_start_hold("unregistered", Wire::Chat, Duration::from_millis(1_234));
        metrics.count_cold_start_hold("unregistered", Wire::Chat, Duration::from_millis(5));
        assert!(
            metrics
                .render()
                .contains("\nllmgw_cold_start_wait_seconds_total 1.239\n")
        );
        for (milliseconds, rendered) in [(0, "0"), (1_000, "1"), (1_500, "1.5"), (20, "0.02")] {
            assert_eq!(super::seconds(milliseconds), rendered);
        }
    }

    #[test]
    fn a_record_on_an_unregistered_route_feeds_the_process_wide_series_only() {
        let metrics = Metrics::default();
        metrics.record(&UsageRecord {
            model: Some("elsewhere".to_owned()),
            wire: Wire::Messages,
            disposition: Disposition::UpstreamFailed,
            refusal: Some(RefusalCode::UpstreamFailed),
            status: 502,
            target_status: None,
            reported_model: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_output_tokens: None,
            response_bytes: 0,
            duration_ms: 1,
        });
        let text = metrics.render();
        assert!(text.contains("\nllmgw_inference_requests_total 1\n"));
        assert!(text.contains("\nllmgw_upstream_failures_total 1\n"));
        assert!(!text.contains("elsewhere"));
    }
}
