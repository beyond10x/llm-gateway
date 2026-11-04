//! What a route's upstream server says about itself, as the embedding last read it: a vLLM
//! server's prefix cache, its queue and its KV cache, and whether it serves.
//!
//! vLLM's answers carry no cache fields, so a call's cache read stays unknown; the server's own
//! counters say how much of the prompts it served from its prefix cache. The hit rate is the
//! hits over the queries counted since the gateway began reading, both in tokens.

use std::fmt::Write as _;

use super::{State, escape, series};
use crate::json::Writer;

/// One route's upstream server, read by the embedding ([`super::Dashboard::set_upstream`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpstreamStats {
    /// What was read: `"vllm"`.
    pub source: String,
    /// When it was last read, in milliseconds since the Unix epoch.
    pub read_ms: u64,
    /// Prompt tokens served from the prefix cache since the gateway began reading.
    pub prefix_cache_hit_tokens: u64,
    /// Prompt tokens looked up in the prefix cache since the gateway began reading.
    pub prefix_cache_query_tokens: u64,
    pub requests_running: Option<f64>,
    pub requests_waiting: Option<f64>,
    /// The share of the KV cache in use, 0 to 1.
    pub kv_cache_usage: Option<f64>,
    /// Whether the server listed its models; `None` when it did not answer.
    pub ready: Option<bool>,
}

impl UpstreamStats {
    /// The prefix cache's hit rate; `None` before any query was counted.
    #[allow(clippy::cast_precision_loss)]
    pub fn hit_ratio(&self) -> Option<f64> {
        (self.prefix_cache_query_tokens > 0)
            .then(|| self.prefix_cache_hit_tokens as f64 / self.prefix_cache_query_tokens as f64)
    }
}

/// The hit rate over every route's upstream, and the tokens hit.
#[allow(clippy::cast_precision_loss)]
pub(super) fn server_cache(state: &State) -> (Option<f64>, u64) {
    let (hits, queries) = state.upstreams.values().fold((0_u64, 0_u64), |(h, q), up| {
        (
            h.saturating_add(up.prefix_cache_hit_tokens),
            q.saturating_add(up.prefix_cache_query_tokens),
        )
    });
    ((queries > 0).then(|| hits as f64 / queries as f64), hits)
}

/// The `upstream` member of a route row.
pub(super) fn record(writer: &mut Writer, upstream: Option<&UpstreamStats>) {
    let Some(up) = upstream else {
        writer.null();
        return;
    };
    super::record(writer, &super::UPSTREAM, |writer, key| match key {
        "source" => writer.string(&up.source),
        "read_ms" => writer.number(up.read_ms),
        "prefix_cache_hit_ratio" => optional(writer, up.hit_ratio()),
        "prefix_cache_hit_tokens" => writer.number(up.prefix_cache_hit_tokens),
        "prefix_cache_query_tokens" => writer.number(up.prefix_cache_query_tokens),
        "requests_running" => optional(writer, up.requests_running),
        "requests_waiting" => optional(writer, up.requests_waiting),
        "kv_cache_usage" => optional(writer, up.kv_cache_usage),
        "ready" => match up.ready {
            Some(ready) => writer.boolean(ready),
            None => writer.null(),
        },
        _ => writer.null(),
    });
}

fn optional(writer: &mut Writer, value: Option<f64>) {
    match value {
        Some(value) => writer.decimal(value),
        None => writer.null(),
    }
}

/// The upstream series, one sample per route whose upstream reported the figure.
pub(super) fn render(out: &mut String, state: &State) {
    type Pick = fn(&UpstreamStats) -> Option<f64>;
    let figures: [(&str, &str, Pick); 4] = [
        (
            "llmgw_upstream_prefix_cache_hit_ratio",
            "The upstream server's prefix cache hits over its queries, in tokens, since the gateway began reading.",
            UpstreamStats::hit_ratio,
        ),
        (
            "llmgw_upstream_requests_running",
            "Requests the upstream server is running, as it last reported.",
            |up| up.requests_running,
        ),
        (
            "llmgw_upstream_requests_waiting",
            "Requests waiting at the upstream server, as it last reported.",
            |up| up.requests_waiting,
        ),
        (
            "llmgw_upstream_kv_cache_usage_ratio",
            "The share of the upstream server's KV cache in use, as it last reported.",
            |up| up.kv_cache_usage,
        ),
    ];
    for (name, help, pick) in figures {
        let name = series(out, name, "gauge", help);
        for (route, up) in &state.upstreams {
            if let Some(value) = pick(up) {
                let _ = writeln!(out, "{name}{{route=\"{}\"}} {value}", escape(route));
            }
        }
    }
}
