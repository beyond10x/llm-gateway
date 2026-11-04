//! The dashboard's headline figures: token totals and their rate over the last minute, the cache
//! hit rate, tokens per second per route, and the subscription limits Anthropic reports in its
//! `anthropic-ratelimit-*` answer headers, kept per credential label.
//!
//! A credential is never kept: its label is the first eight hex digits of the SHA-256 of the
//! value the client sent, which tells two subscriptions apart and cannot be turned back.

use std::{collections::BTreeMap, fmt::Write as _};

use super::{Attempt, CallRow, RouteTotals, State, escape, rate, series};
use crate::json::Writer;

/// How far back the current rates look, in milliseconds.
const RATE_WINDOW_MS: u64 = 60_000;
/// Credentials whose limits are kept; past it the one seen longest ago is dropped.
pub const MAX_CREDENTIALS: usize = 16;
/// Limit headers kept per credential.
const MAX_LIMIT_HEADERS: usize = 64;
/// The prefix of every limit header kept.
const LIMIT_PREFIX: &str = "anthropic-ratelimit-";
/// The bounds of the tokens-per-second histograms.
const TPS_BOUNDS: [u64; 10] = [10, 25, 50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000];

/// The limits last reported for one credential.
#[derive(Debug, Clone, Default)]
pub(super) struct LimitEntry {
    seen_ms: u64,
    headers: BTreeMap<String, String>,
}

/// A histogram of tokens per second.
#[derive(Debug, Clone, Default)]
pub(super) struct TpsHistogram {
    counts: [u64; TPS_BOUNDS.len()],
    count: u64,
    sum: f64,
}

impl TpsHistogram {
    pub(super) fn observe(&mut self, tps: f64) {
        for (bound, count) in TPS_BOUNDS.iter().zip(self.counts.iter_mut()) {
            #[allow(clippy::cast_precision_loss)]
            if tps <= *bound as f64 {
                *count += 1;
            }
        }
        self.count += 1;
        self.sum += tps;
    }

    /// The mean of the observed rates; none before the first.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn mean(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// The label of a credential value: the first eight hex digits of its SHA-256.
pub(crate) fn credential_label(value: &[u8]) -> String {
    let digest = crate::auth::sha256(value);
    let mut label = String::with_capacity(8);
    for byte in &digest[..4] {
        let _ = write!(label, "{byte:02x}");
    }
    label
}

/// Keeps the limit headers of the last attempt that carried any, for `credential`.
pub(super) fn note_limits(
    limits: &mut BTreeMap<String, LimitEntry>,
    credential: Option<&str>,
    attempts: &[Attempt],
    now: u64,
) {
    let Some(credential) = credential else {
        return;
    };
    let Some(attempt) = attempts.iter().rev().find(|attempt| {
        attempt
            .headers
            .iter()
            .any(|(name, _)| name.starts_with(LIMIT_PREFIX))
    }) else {
        return;
    };
    let headers = attempt
        .headers
        .iter()
        .filter(|(name, _)| name.starts_with(LIMIT_PREFIX))
        .take(MAX_LIMIT_HEADERS)
        .cloned()
        .collect();
    limits.insert(
        credential.to_owned(),
        LimitEntry {
            seen_ms: now,
            headers,
        },
    );
    while limits.len() > MAX_CREDENTIALS {
        let Some(oldest) = limits
            .iter()
            .min_by_key(|(_, entry)| entry.seen_ms)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        limits.remove(&oldest);
    }
}

/// A call's input tokens that no cache served: on the messages wire `input_tokens` already
/// leaves the cached ones out; the other wires count them in.
pub(super) fn uncached(call_input: Option<u64>, cache_read: Option<u64>, messages: bool) -> u64 {
    let input = call_input.unwrap_or(0);
    if messages {
        input
    } else {
        input.saturating_sub(cache_read.unwrap_or(0))
    }
}

/// The cache hit rate: cached reads over every input token the model was given.
#[allow(clippy::cast_precision_loss)]
pub fn hit_rate(cache_read: u64, cache_creation: u64, uncached: u64) -> Option<f64> {
    let total = cache_read + cache_creation + uncached;
    (total > 0).then(|| cache_read as f64 / total as f64)
}

/// Totals over every route.
struct Totals {
    uncached: u64,
    cache_read: u64,
    cache_creation: u64,
    output: u64,
}

fn totals(state: &State) -> Totals {
    let mut totals = Totals {
        uncached: 0,
        cache_read: 0,
        cache_creation: 0,
        output: 0,
    };
    for route in state.routes.values() {
        totals.uncached += route.uncached_input_tokens;
        totals.cache_read += route.cache_read_tokens;
        totals.cache_creation += route.cache_creation_tokens;
        totals.output += route.output_tokens;
    }
    totals
}

fn recent(state: &State, now: u64) -> impl Iterator<Item = &CallRow> {
    state.calls.iter().filter(move |call| {
        now.saturating_sub(call.started_ms + call.duration_ms) <= RATE_WINDOW_MS
    })
}

fn input_of(call: &CallRow) -> u64 {
    super::total_input(&call.usage, call.wire).unwrap_or(0)
}

/// Tokens per second over the last minute, in and out; `None` with no call in it.
#[allow(clippy::cast_precision_loss)]
fn window_rates(state: &State, now: u64) -> (Option<f64>, Option<f64>) {
    let mut any = false;
    let (mut input, mut output) = (0_u64, 0_u64);
    for call in recent(state, now) {
        any = true;
        input += input_of(call);
        output += call.usage.output_tokens.unwrap_or(0);
    }
    let seconds = RATE_WINDOW_MS as f64 / 1000.0;
    if any {
        (Some(input as f64 / seconds), Some(output as f64 / seconds))
    } else {
        (None, None)
    }
}

/// Per route, the tokens per second of its calls in the last minute.
fn route_rates(state: &State, now: u64) -> BTreeMap<String, (Option<f64>, Option<f64>)> {
    let mut sums: BTreeMap<String, [u64; 4]> = BTreeMap::new();
    for call in recent(state, now) {
        let Some(route) = &call.route else { continue };
        let sum = sums.entry(route.clone()).or_default();
        if let Some((tokens, ttft)) = super::input_sample(&call.usage, call.wire, call.ttft_ms) {
            sum[0] += tokens;
            sum[1] += ttft;
        }
        if let Some((tokens, streaming)) =
            super::output_sample(&call.usage, call.ttft_ms, call.duration_ms)
        {
            sum[2] += tokens;
            sum[3] += streaming;
        }
    }
    sums.into_iter()
        .map(|(route, sum)| (route, (rate(sum[0], sum[1]), rate(sum[2], sum[3]))))
        .collect()
}

fn optional_decimal(writer: &mut Writer, value: Option<f64>) {
    match value {
        Some(value) => writer.decimal(value),
        None => writer.null(),
    }
}

/// The `kpi` member of the snapshot.
pub(super) fn kpi_record(writer: &mut Writer, state: &State, now: u64) {
    let totals = totals(state);
    let (input_rate, output_rate) = window_rates(state, now);
    super::record(writer, &super::KPI, |writer, key| match key {
        "input_tokens" => {
            writer.number(totals.uncached + totals.cache_read + totals.cache_creation);
        }
        "output_tokens" => writer.number(totals.output),
        "input_rate" => optional_decimal(writer, input_rate),
        "output_rate" => optional_decimal(writer, output_rate),
        "cache_read_tokens" => writer.number(totals.cache_read),
        "cache_creation_tokens" => writer.number(totals.cache_creation),
        "uncached_input_tokens" => writer.number(totals.uncached),
        "cache_hit_rate" => optional_decimal(
            writer,
            hit_rate(totals.cache_read, totals.cache_creation, totals.uncached),
        ),
        "route_rates" => {
            writer.begin_array();
            for (route, (input, output)) in route_rates(state, now) {
                super::record(writer, &super::ROUTE_RATE, |writer, key| match key {
                    "route" => writer.string(&route),
                    "input_tps" => optional_decimal(writer, input),
                    "output_tps" => optional_decimal(writer, output),
                    _ => writer.null(),
                });
            }
            writer.end_array();
        }
        _ => writer.null(),
    });
}

/// The `limits` member of the snapshot.
pub(super) fn limits_record(writer: &mut Writer, state: &State) {
    writer.begin_array();
    for (credential, entry) in &state.limits {
        super::record(writer, &super::LIMIT, |writer, key| match key {
            "credential" => writer.string(credential),
            "seen_ms" => writer.number(entry.seen_ms),
            "headers" => {
                writer.begin_object();
                for (name, value) in &entry.headers {
                    writer.key(name);
                    writer.string(value);
                }
                writer.end_object();
            }
            _ => writer.null(),
        });
    }
    writer.end_array();
}

fn gauge(out: &mut String, name: &str, labels: &str, value: f64) {
    let _ = writeln!(out, "{name}{{{labels}}} {value}");
}

/// The KPI, throughput and subscription-limit series.
pub(super) fn render(out: &mut String, state: &State, now: u64) {
    let totals = totals(state);
    let hit = series(
        out,
        "llmgw_cache_hit_ratio",
        "gauge",
        "Cache reads over every input token (cache read + cache creation + uncached input).",
    );
    if let Some(ratio) = hit_rate(totals.cache_read, totals.cache_creation, totals.uncached) {
        let _ = writeln!(out, "{hit} {ratio}");
    }
    let (input, output) = window_rates(state, now);
    let rate = series(
        out,
        "llmgw_tokens_per_second",
        "gauge",
        "Tokens per second over the last minute, by direction; written once a call finished.",
    );
    if !state.routes.is_empty() {
        gauge(out, &rate, "direction=\"in\"", input.unwrap_or(0.0));
        gauge(out, &rate, "direction=\"out\"", output.unwrap_or(0.0));
    }
    for (name, help, pick) in [
        (
            "llmgw_input_tps",
            "Input tokens per second of each call, per route.",
            (|totals: &RouteTotals| &totals.input_tps) as fn(&RouteTotals) -> &TpsHistogram,
        ),
        (
            "llmgw_output_tps",
            "Output tokens per second of each call, per route.",
            |totals: &RouteTotals| &totals.output_tps,
        ),
    ] {
        let name = series(out, name, "histogram", help);
        for (route, totals) in &state.routes {
            let route = escape(route);
            let histogram = pick(totals);
            for (bound, count) in TPS_BOUNDS.iter().zip(histogram.counts) {
                let _ = writeln!(
                    out,
                    "{name}_bucket{{route=\"{route}\",le=\"{bound}\"}} {count}"
                );
            }
            let _ = writeln!(
                out,
                "{name}_bucket{{route=\"{route}\",le=\"+Inf\"}} {}",
                histogram.count
            );
            let _ = writeln!(out, "{name}_sum{{route=\"{route}\"}} {}", histogram.sum);
            let _ = writeln!(out, "{name}_count{{route=\"{route}\"}} {}", histogram.count);
        }
    }
    render_limits(out, state, now);
}

fn render_limits(out: &mut String, state: &State, now: u64) {
    let utilization = series(
        out,
        "llmgw_anthropic_utilization",
        "gauge",
        "Subscription utilization Anthropic last reported, per window and credential label.",
    );
    let reset = series(
        out,
        "llmgw_anthropic_reset_seconds",
        "gauge",
        "Seconds until the subscription window resets, per window and credential label.",
    );
    let status = series(
        out,
        "llmgw_anthropic_status",
        "gauge",
        "1 for the status Anthropic last reported, per window (unified, 5h, 7d, overage) and credential label.",
    );
    for (credential, entry) in &state.limits {
        let credential = escape(credential);
        let header = |suffix: &str| entry.headers.get(&format!("{LIMIT_PREFIX}unified{suffix}"));
        for window in ["5h", "7d"] {
            let labels = format!("window=\"{window}\",credential=\"{credential}\"");
            if let Some(value) = header(&format!("-{window}-utilization"))
                .and_then(|value| value.parse::<f64>().ok())
            {
                gauge(out, &utilization, &labels, value);
            }
            if let Some(at) =
                header(&format!("-{window}-reset")).and_then(|value| value.parse::<u64>().ok())
            {
                let _ = writeln!(out, "{reset}{{{labels}}} {}", at.saturating_sub(now / 1000));
            }
        }
        for (window, suffix) in [
            ("unified", "-status"),
            ("5h", "-5h-status"),
            ("7d", "-7d-status"),
            ("overage", "-overage-status"),
        ] {
            if let Some(value) = header(suffix) {
                let _ = writeln!(
                    out,
                    "{status}{{window=\"{window}\",credential=\"{credential}\",status=\"{}\"}} 1",
                    escape(value)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{credential_label, hit_rate, uncached};

    #[test]
    fn the_hit_rate_is_cache_reads_over_every_input_token() {
        assert_eq!(hit_rate(0, 0, 0), None);
        assert_eq!(hit_rate(600, 100, 300), Some(0.6));
        assert_eq!(hit_rate(0, 50, 50), Some(0.0));
        assert_eq!(uncached(Some(300), Some(600), true), 300);
        assert_eq!(uncached(Some(900), Some(600), false), 300);
    }

    #[test]
    fn a_credential_label_is_eight_hex_digits_and_not_the_value() {
        let label = credential_label(b"Bearer sk-ant-oat01-secret");
        assert_eq!(label.len(), 8);
        assert!(label.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(label, credential_label(b"Bearer sk-ant-oat01-other"));
    }
}
