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

/// How one model call ended, as `llmgw_requests_total` labels it (`llm-gateway.telemetry.
/// Outcome`). The table from every refusal code to one outcome is published in
/// `docs/gateway.md`, and checked against it here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    /// An answer reached the client, whatever its status.
    Ok,
    /// The target failed, refused the call or closed without an answer.
    UpstreamError,
    /// The target's read timeout passed before its answer head, or a start deadline passed.
    Timeout,
    /// The gateway refused the call before reaching a target.
    Refused,
}

impl Outcome {
    /// Every outcome, in the order a scrape lists them.
    pub const ALL: [Self; 4] = [Self::Ok, Self::UpstreamError, Self::Timeout, Self::Refused];

    /// The `outcome` label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::UpstreamError => "upstream_error",
            Self::Timeout => "timeout",
            Self::Refused => "refused",
        }
    }

    /// The outcome of a call answered `refusal`, or relayed when it is `None`. `timed_out` says
    /// the target's read timed out before its answer head arrived, which makes an
    /// `upstream-failed` a timeout.
    pub const fn of(refusal: Option<RefusalCode>, timed_out: bool) -> Self {
        match refusal {
            None => Self::Ok,
            Some(RefusalCode::ModelColdStart) => Self::Timeout,
            Some(RefusalCode::UpstreamFailed) if timed_out => Self::Timeout,
            Some(
                RefusalCode::UpstreamFailed
                | RefusalCode::UpstreamRefused
                | RefusalCode::TranslationUnrepresentable
                | RefusalCode::TranslationTooLarge,
            ) => Self::UpstreamError,
            Some(_) => Self::Refused,
        }
    }
}

/// What the gateway learned about one model call it was asked to relay
/// (`llm-gateway.telemetry.UsageRecord`). One is made for every request to a wire path that
/// passed owner authentication, or needed none (`IngressAuth::None`), once its answer or
/// refusal is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// The relayed model's alias the body named. `None` when it named no relayed model.
    pub model: Option<String>,
    pub wire: Wire,
    pub disposition: Disposition,
    /// How the call ended for `llmgw_requests_total`.
    pub outcome: Outcome,
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
    /// The profile of the pod that served the call. `None` for a hosted endpoint, a target
    /// that names no profile, and a call no target served.
    pub profile: Option<String>,
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

/// The nine process-wide series: llmgw's eight in llmgw's order (matrix row O1), then the
/// gateway's own spend-refusal series.
const PROCESS: [Series; 9] = [
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
    Series {
        name: "llmgw_pod_start_spend_refusals_total",
        help: "Pod starts refused because they would pass a spend ceiling or the spend is unknown.",
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

/// The two per-model series, after the route series (`llm-gateway.telemetry`).
const MODEL: [Series; 2] = [
    Series {
        name: "llmgw_requests_total",
        help: "Model calls per model, by how they ended.",
    },
    Series {
        name: "llmgw_request_duration_seconds",
        help: "Seconds from the end of a model call's request head to the last byte written, per model.",
    },
];

/// The histogram's finite bucket bounds in milliseconds, published in `docs/gateway.md`; a
/// record falls in every bucket whose bound is at least its `duration_ms`.
const BUCKETS_MS: [u64; 12] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000, 60_000, 120_000, 300_000,
];

/// One model's outcome counters and duration histogram.
#[derive(Debug, Default, Clone, Copy)]
struct ModelSeries {
    outcomes: [u64; 4],
    /// Cumulative: each bucket counts every record at or under its bound.
    buckets: [u64; 12],
    sum_ms: u64,
    count: u64,
}

impl ModelSeries {
    fn observe(&mut self, record: &UsageRecord) {
        let outcome = &mut self.outcomes[record.outcome as usize];
        *outcome = outcome.saturating_add(1);
        for (bucket, bound) in self.buckets.iter_mut().zip(BUCKETS_MS) {
            if record.duration_ms <= bound {
                *bucket = bucket.saturating_add(1);
            }
        }
        self.sum_ms = self.sum_ms.saturating_add(record.duration_ms);
        self.count = self.count.saturating_add(1);
    }
}

/// A process-wide counter, by its place in [`PROCESS`].
#[derive(Clone, Copy)]
enum Process {
    InferenceRequests,
    UpstreamFailures,
    InstructionViews,
    PodStarts,
    PodStartFailures,
    PodReaps,
    EndpointInvalidations,
    /// Kept in milliseconds and rendered in seconds.
    ColdStartWait,
    PodStartSpendRefusals,
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
    process: [AtomicU64; 9],
    /// Per (alias, wire) a relayed model declares, registered at zero when the relay is
    /// composed. A call on any other pair feeds the process-wide series only.
    routes: Mutex<BTreeMap<(String, Wire), [u64; 5]>>,
    /// Per relayed model, registered at zero when the relay is composed. A record naming no
    /// registered model feeds none of them.
    models: Mutex<BTreeMap<String, ModelSeries>>,
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

    /// Counts a pod start the spend check refused, answered `spend-ceiling`
    /// (`llmgw_pod_start_spend_refusals_total`).
    pub fn count_pod_start_spend_refusal(&self) {
        self.add(Process::PodStartSpendRefusals, 1);
    }

    /// Counts deployments and pods a cleanup pass stopped (`llmgw_pod_reaps_total`).
    pub fn count_pod_reaps(&self, stopped: u64) {
        self.add(Process::PodReaps, stopped);
    }

    /// Counts a setup instructions answer served at `GET /` (`llmgw_instruction_views_total`,
    /// row R1).
    pub(crate) fn count_instruction_view(&self) {
        self.add(Process::InstructionViews, 1);
    }

    /// The counters as Prometheus text (format 0.0.4): one `# HELP` and `# TYPE` pair and its
    /// samples per series, process-wide first, each route in alias and wire order, then each
    /// model's outcome counter and duration histogram in alias order.
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (index, series) in PROCESS.iter().enumerate() {
            header(&mut text, series, "counter");
            let value = self.process[index].load(Ordering::SeqCst);
            if index == Process::ColdStartWait as usize {
                let _ = writeln!(text, "{} {}", series.name, seconds(value));
            } else {
                let _ = writeln!(text, "{} {value}", series.name);
            }
        }
        let routes = self.routes();
        for (index, series) in ROUTE.iter().enumerate() {
            header(&mut text, series, "counter");
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
        drop(routes);
        let models = self.models();
        let [requests, duration] = &MODEL;
        header(&mut text, requests, "counter");
        for (alias, model) in models.iter() {
            for outcome in Outcome::ALL {
                let _ = writeln!(
                    text,
                    "{}{{model=\"{}\",outcome=\"{}\"}} {}",
                    requests.name,
                    escape(alias),
                    outcome.label(),
                    model.outcomes[outcome as usize]
                );
            }
        }
        header(&mut text, duration, "histogram");
        for (alias, model) in models.iter() {
            let alias = escape(alias);
            let name = duration.name;
            for (bound, value) in BUCKETS_MS.iter().zip(model.buckets) {
                let le = seconds(*bound);
                let _ = writeln!(
                    text,
                    "{name}_bucket{{model=\"{alias}\",le=\"{le}\"}} {value}"
                );
            }
            let _ = writeln!(
                text,
                "{name}_bucket{{model=\"{alias}\",le=\"+Inf\"}} {}",
                model.count
            );
            let _ = writeln!(
                text,
                "{name}_sum{{model=\"{alias}\"}} {}",
                seconds(model.sum_ms)
            );
            let _ = writeln!(text, "{name}_count{{model=\"{alias}\"}} {}", model.count);
        }
        text
    }

    /// Registers a (model, wire) pair at zero, and the model's own series. Registering one
    /// twice changes nothing.
    pub(crate) fn register(&self, alias: &str, wire: Wire) {
        self.routes()
            .entry((alias.to_string(), wire))
            .or_insert([0; 5]);
        self.models().entry(alias.to_string()).or_default();
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
        if let Some(model) = self.models().get_mut(alias) {
            model.observe(record);
        }
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

    fn models(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, ModelSeries>> {
        self.models.lock().unwrap_or_else(PoisonError::into_inner)
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

fn header(text: &mut String, series: &Series, kind: &str) {
    let _ = writeln!(text, "# HELP {} {}", series.name, series.help);
    let _ = writeln!(text, "# TYPE {} {kind}", series.name);
}

/// A label value as the text format writes it: `\` and `"` escaped. An alias is printable
/// US-ASCII (`Label`), so it holds no line feed.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::{BUCKETS_MS, Disposition, Metrics, Outcome, UsageRecord};
    use crate::{error::RefusalCode, relay::Wire};
    use std::time::Duration;

    const CONTRACT: &str = include_str!("../../../docs/gateway.md");

    /// The `### <title>` subsection of `docs/gateway.md`, up to the next heading.
    fn subsection(title: &str) -> &'static str {
        CONTRACT
            .split_once(&format!("\n### {title}\n"))
            .unwrap_or_else(|| panic!("docs/gateway.md has no {title} subsection"))
            .1
            .split("\n#")
            .next()
            .unwrap_or_default()
    }

    /// The backticked words of `text`, in order.
    fn quoted(text: &str) -> Vec<&str> {
        text.split('`').skip(1).step_by(2).collect()
    }

    fn record(model: &str, refusal: Option<RefusalCode>, duration_ms: u64) -> UsageRecord {
        UsageRecord {
            model: Some(model.to_owned()),
            wire: Wire::Chat,
            disposition: if refusal.is_some() {
                Disposition::Refused
            } else {
                Disposition::Relayed
            },
            outcome: Outcome::of(refusal, false),
            refusal,
            status: refusal.map_or(200, RefusalCode::status),
            target_status: None,
            reported_model: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_output_tokens: None,
            response_bytes: 0,
            duration_ms,
            profile: None,
        }
    }

    fn sample(text: &str, series: &str) -> String {
        text.lines()
            .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
            .unwrap_or_else(|| panic!("no sample {series} in\n{text}"))
            .to_owned()
    }

    #[test]
    fn the_published_buckets_are_the_histograms_and_each_flips_at_its_bound() {
        let published: Vec<&str> = quoted(
            subsection("Latency and outcomes per model")
                .lines()
                .find(|line| line.starts_with("The histogram's buckets are"))
                .expect("docs/gateway.md publishes no buckets"),
        );
        let rendered: Vec<String> = BUCKETS_MS
            .iter()
            .map(|bound| super::seconds(*bound))
            .collect();
        let mut expected: Vec<&str> = rendered.iter().map(String::as_str).collect();
        expected.push("+Inf");
        assert_eq!(published, expected);
        for (index, bound) in BUCKETS_MS.iter().enumerate() {
            let metrics = Metrics::default();
            metrics.register("m", Wire::Chat);
            metrics.record(&record("m", None, *bound));
            metrics.record(&record("m", None, bound + 1));
            let text = metrics.render();
            let le = |at: usize| {
                let series = format!(
                    "llmgw_request_duration_seconds_bucket{{model=\"m\",le=\"{}\"}}",
                    rendered[at]
                );
                sample(&text, &series)
            };
            // At its bound a record is in the bucket; one millisecond over it is not.
            assert_eq!(le(index), "1", "le={}", rendered[index]);
            if index > 0 {
                assert_eq!(le(index - 1), "0", "le={}", rendered[index - 1]);
            }
            if index + 1 < BUCKETS_MS.len() {
                assert_eq!(le(index + 1), "2", "le={}", rendered[index + 1]);
            }
            let inf = r#"llmgw_request_duration_seconds_bucket{model="m",le="+Inf"}"#;
            assert_eq!(sample(&text, inf), "2");
            let sum = r#"llmgw_request_duration_seconds_sum{model="m"}"#;
            assert_eq!(sample(&text, sum), super::seconds(2 * bound + 1));
        }
    }

    #[test]
    fn the_published_outcome_table_names_every_refusal_code_with_its_outcome() {
        let rows: Vec<(&str, Vec<&str>)> = subsection("Call outcomes")
            .lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| {
                let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
                assert_eq!(cells.len(), 2, "unexpected outcome row: {line}");
                (cells[0].trim_matches('`'), quoted(cells[1]))
            })
            .collect();
        assert_eq!(
            rows.len(),
            RefusalCode::ALL.len(),
            "one row per refusal code"
        );
        for code in RefusalCode::ALL {
            let (_, outcomes) = rows
                .iter()
                .find(|(published, _)| *published == code.wire())
                .unwrap_or_else(|| panic!("{} has no outcome row", code.wire()));
            let plain = Outcome::of(Some(*code), false).label();
            let timed_out = Outcome::of(Some(*code), true).label();
            assert_eq!(outcomes.first(), Some(&plain), "{}", code.wire());
            let mut expected = vec![plain];
            if timed_out != plain {
                expected.push(timed_out);
            }
            assert_eq!(*outcomes, expected, "{}", code.wire());
        }
        assert_eq!(Outcome::of(None, false), Outcome::Ok);
        assert_eq!(Outcome::of(None, true), Outcome::Ok);
        let labels: Vec<&str> = Outcome::ALL.iter().map(|each| each.label()).collect();
        assert_eq!(labels, ["ok", "upstream_error", "timeout", "refused"]);
    }

    #[test]
    fn a_registered_model_counts_its_outcomes_and_an_unregistered_one_none() {
        let metrics = Metrics::default();
        metrics.register("m", Wire::Chat);
        metrics.record(&record("m", Some(RefusalCode::WireNotServed), 7));
        metrics.record(&record("elsewhere", None, 7));
        let text = metrics.render();
        assert_eq!(
            sample(
                &text,
                r#"llmgw_requests_total{model="m",outcome="refused"}"#
            ),
            "1"
        );
        assert_eq!(
            sample(&text, r#"llmgw_request_duration_seconds_count{model="m"}"#),
            "1"
        );
        assert!(!text.contains("elsewhere"));
    }

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
            outcome: Outcome::UpstreamError,
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
            profile: None,
        });
        let text = metrics.render();
        assert!(text.contains("\nllmgw_inference_requests_total 1\n"));
        assert!(text.contains("\nllmgw_upstream_failures_total 1\n"));
        assert!(!text.contains("elsewhere"));
    }
}
