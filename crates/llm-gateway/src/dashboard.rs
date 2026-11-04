//! The live dashboard (story:live-dashboard): every finished call as a record, the calls in
//! flight, per-session token rates, per-route totals and the provider panel the embedding sets,
//! served as one page (`GET /dashboard`), a JSON snapshot (`GET /dashboard/snapshot`), server-sent
//! events (`GET /dashboard/events`) and Prometheus series appended to `GET /metrics`.
//!
//! Every record type is declared once, as a list of fields with their TypeScript types
//! ([`SNAPSHOT_TYPES`]). The snapshot is written by walking those lists, so a field cannot reach
//! the wire without being declared, and `llm-gateway-docs` generates the page's TypeScript types
//! from the same lists (`dashboard/src/generated/snapshot.ts`, drift-checked by `task docs`).
//!
//! Nothing here holds a credential, a prompt or an answer. A session is named by a hash of the
//! header that identifies it (or of the user agent and client address), never by the header's
//! value; a user agent is kept as printable ASCII of at most [`MAX_USER_AGENT_BYTES`].

use crate::{
    error::RefusalCode,
    json::Writer,
    metrics::{Outcome, UsageRecord},
    relay::Wire,
    usage::Usage,
};
mod kpi;
mod upstream;

pub(crate) use kpi::credential_label;
pub use kpi::{MAX_CREDENTIALS as MAX_LIMIT_CREDENTIALS, hit_rate as cache_hit_rate};
pub use upstream::UpstreamStats;

use std::{
    collections::{BTreeMap, VecDeque},
    fmt::Write as _,
    net::IpAddr,
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// The snapshot's `format`.
pub const SNAPSHOT_FORMAT: &str = "llm-gateway.dashboard/1";
/// The most finished calls a snapshot lists, newest first.
pub const MAX_CALLS: usize = 200;
/// The most sessions kept; the one seen longest ago is dropped first.
pub const MAX_SESSIONS: usize = 64;
/// The longest user agent kept, in bytes.
pub const MAX_USER_AGENT_BYTES: usize = 128;
/// How long a body digest is remembered for the repeated-body flag.
pub const REPEAT_WINDOW: Duration = Duration::from_secs(600);
/// How many body digests are remembered.
const DIGESTS: usize = 64;
/// The bytes of a session hash shown, as hex.
const SESSION_HEX: usize = 12;

/// One record type of the snapshot: its TypeScript name and its fields, each a JSON key and the
/// TypeScript type of its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordType {
    pub name: &'static str,
    pub fields: &'static [(&'static str, &'static str)],
}

const WIRE_TS: &str = "\"chat\" | \"responses\" | \"messages\"";

const SNAPSHOT: RecordType = RecordType {
    name: "Snapshot",
    fields: &[
        ("format", "\"llm-gateway.dashboard/1\""),
        ("now_ms", "number"),
        ("started_ms", "number"),
        ("calls", "CallRecord[]"),
        ("in_flight", "InFlight[]"),
        ("sessions", "SessionRow[]"),
        ("routes", "RouteRow[]"),
        ("provider", "ProviderPanel | null"),
        ("kpi", "Kpi"),
        ("limits", "LimitRow[]"),
    ],
};

const CALL: RecordType = RecordType {
    name: "CallRecord",
    fields: &[
        ("id", "number"),
        ("started_ms", "number"),
        ("route", "string | null"),
        ("upstream", "string | null"),
        ("upstream_model", "string | null"),
        ("wire", WIRE_TS),
        ("status", "number"),
        ("target_status", "number | null"),
        (
            "outcome",
            "\"ok\" | \"upstream_error\" | \"timeout\" | \"refused\"",
        ),
        ("refusal", "string | null"),
        ("queue_wait_ms", "number | null"),
        ("ttft_ms", "number | null"),
        ("duration_ms", "number"),
        ("response_bytes", "number"),
        ("input_tokens", "number | null"),
        ("output_tokens", "number | null"),
        ("cache_read_tokens", "number | null"),
        ("cache_creation_tokens", "number | null"),
        ("input_tps", "number | null"),
        ("output_tps", "number | null"),
        ("usage_note", "string | null"),
        ("client_retry", "number | null"),
        ("repeated_body", "boolean"),
        ("client_disconnected", "boolean"),
        ("upstream_retry_after", "string | null"),
        ("attempts", "AttemptRecord[]"),
        ("session", "string"),
        ("user_agent", "string | null"),
    ],
};

const ATTEMPT: RecordType = RecordType {
    name: "AttemptRecord",
    fields: &[
        ("attempt", "number"),
        ("status", "number | null"),
        ("duration_ms", "number"),
        ("wait_ms", "number"),
        ("retry_after", "string | null"),
        ("headers", "Record<string, string>"),
        ("error_body", "string | null"),
    ],
};

const IN_FLIGHT: RecordType = RecordType {
    name: "InFlight",
    fields: &[
        ("id", "number"),
        ("started_ms", "number"),
        ("route", "string | null"),
        ("wire", WIRE_TS),
        (
            "state",
            "\"waiting-upstream\" | \"waiting-retry\" | \"streaming\"",
        ),
        ("attempt", "number"),
        ("retry_at_ms", "number | null"),
        ("elapsed_ms", "number"),
        ("session", "string"),
    ],
};

const SESSION: RecordType = RecordType {
    name: "SessionRow",
    fields: &[
        ("session", "string"),
        ("label", "string"),
        ("calls", "number"),
        ("input_tokens", "number"),
        ("output_tokens", "number"),
        ("cache_read_tokens", "number"),
        ("input_tps", "number | null"),
        ("output_tps", "number | null"),
        ("last_ms", "number"),
    ],
};

const ROUTE: RecordType = RecordType {
    name: "RouteRow",
    fields: &[
        ("route", "string"),
        ("calls", "number"),
        ("errors", "number"),
        ("input_tokens", "number"),
        ("output_tokens", "number"),
        ("cache_read_tokens", "number"),
        ("cache_creation_tokens", "number"),
        ("uncached_input_tokens", "number"),
        ("input_tps", "number | null"),
        ("output_tps", "number | null"),
        ("in_flight", "number"),
        ("upstream_model", "string | null"),
        ("upstream", "UpstreamRow | null"),
    ],
};

const UPSTREAM: RecordType = RecordType {
    name: "UpstreamRow",
    fields: &[
        ("source", "string"),
        ("read_ms", "number"),
        ("prefix_cache_hit_ratio", "number | null"),
        ("prefix_cache_hit_tokens", "number"),
        ("prefix_cache_query_tokens", "number"),
        ("requests_running", "number | null"),
        ("requests_waiting", "number | null"),
        ("kv_cache_usage", "number | null"),
        ("ready", "boolean | null"),
    ],
};

const PROVIDER: RecordType = RecordType {
    name: "ProviderPanel",
    fields: &[
        ("name", "string"),
        ("read_ms", "number"),
        ("balance_usd", "number | null"),
        ("spend_per_hour_usd", "number | null"),
        ("other_pods", "number"),
        ("pods", "PodRow[]"),
        ("watchdog_heartbeat_age_ms", "number | null"),
    ],
};

const POD: RecordType = RecordType {
    name: "PodRow",
    fields: &[
        ("id", "string"),
        ("name", "string"),
        ("model", "string | null"),
        ("state", "string"),
        ("gpu", "string | null"),
        ("cost_per_hour_usd", "number | null"),
        ("uptime_s", "number | null"),
        ("vllm_ready", "boolean | null"),
        ("last_call_ms", "number | null"),
        ("in_flight", "number"),
        ("shutdown", "PodShutdown | null"),
        ("external", "boolean"),
    ],
};

const POD_SHUTDOWN: RecordType = RecordType {
    name: "PodShutdown",
    fields: &[
        ("at_ms", "number"),
        ("limit", "\"idle\" | \"max-runtime\" | \"balance\""),
    ],
};

const KPI: RecordType = RecordType {
    name: "Kpi",
    fields: &[
        ("input_tokens", "number"),
        ("output_tokens", "number"),
        ("input_rate", "number | null"),
        ("output_rate", "number | null"),
        ("cache_read_tokens", "number"),
        ("cache_creation_tokens", "number"),
        ("uncached_input_tokens", "number"),
        ("cache_hit_rate", "number | null"),
        ("server_cache_hit_rate", "number | null"),
        ("server_cache_hit_tokens", "number"),
        ("route_rates", "RouteRate[]"),
    ],
};

const ROUTE_RATE: RecordType = RecordType {
    name: "RouteRate",
    fields: &[
        ("route", "string"),
        ("input_tps", "number | null"),
        ("output_tps", "number | null"),
    ],
};

const LIMIT: RecordType = RecordType {
    name: "LimitRow",
    fields: &[
        ("credential", "string"),
        ("seen_ms", "number"),
        ("headers", "Record<string, string>"),
    ],
};

/// Every record type of the snapshot, in the order the generated TypeScript declares them.
pub const SNAPSHOT_TYPES: [RecordType; 13] = [
    SNAPSHOT,
    CALL,
    ATTEMPT,
    IN_FLIGHT,
    SESSION,
    ROUTE,
    UPSTREAM,
    PROVIDER,
    POD,
    POD_SHUTDOWN,
    KPI,
    ROUTE_RATE,
    LIMIT,
];

/// The watchdog's limits, for each owned pod's shutdown countdown ([`Dashboard::set_watchdog`]).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WatchdogLimits {
    /// A pod with no call for this long is stopped.
    pub idle_ms: Option<u64>,
    /// A pod is stopped after running this long.
    pub max_runtime_ms: Option<u64>,
    /// Pods are stopped when the balance falls under this many USD; the countdown spends the
    /// balance down at the current spend rate.
    pub min_balance_usd: Option<f64>,
}

/// The milliseconds since the Unix epoch now.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The provider panel the embedding sets ([`Dashboard::set_provider`]): what it last read from
/// its provider's account. The dashboard adds each pod's last call and in-flight count from its
/// own route totals, by [`PodPanel::model`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderPanel {
    pub name: String,
    /// When the account was read, in milliseconds since the Unix epoch.
    pub read_ms: u64,
    pub balance_usd: Option<f64>,
    pub spend_per_hour_usd: Option<f64>,
    /// The account's pods this gateway does not own.
    pub other_pods: u64,
    pub pods: Vec<PodPanel>,
    /// When the watchdog last wrote its heartbeat, in milliseconds since the Unix epoch.
    pub watchdog_heartbeat_ms: Option<u64>,
}

/// One pod the gateway owns, as the embedding read it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PodPanel {
    pub id: String,
    pub name: String,
    /// The relayed model (alias) the pod serves.
    pub model: Option<String>,
    pub state: String,
    pub gpu: Option<String>,
    pub cost_per_hour_usd: Option<f64>,
    pub uptime_s: Option<u64>,
    pub vllm_ready: Option<bool>,
    /// How long the pod may go without a call before it is stopped, when its model says;
    /// otherwise the watchdog's [`WatchdogLimits::idle_ms`] applies.
    pub idle_ms: Option<u64>,
    /// A pod one of the gateway's routes reaches that the gateway does not own: shown, never
    /// stopped, with no countdown.
    pub external: bool,
}

/// What a relayed model was last doing, for a heartbeat ([`Dashboard::route_activity`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteActivity {
    pub route: String,
    /// When its last call finished, in milliseconds since the Unix epoch.
    pub last_call_end_ms: Option<u64>,
    pub in_flight: u64,
}

/// Who sent a call, as far as the dashboard tells: a session hash and a label, the user agent,
/// and the client's own retry count. Read from the request head.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Observed {
    pub(crate) session: String,
    pub(crate) label: String,
    pub(crate) user_agent: Option<String>,
    pub(crate) client_retry: Option<u64>,
    /// No header named the session: it is keyed by user agent and client address, and
    /// [`Observed::at`] supplies the address.
    by_address: bool,
}

impl Observed {
    /// The session of a request: Claude Code's `x-claude-code-session-id`, else Codex's
    /// `session-id` (or `session_id`), else the `x-opencode-session` line, else the user agent
    /// and the client's address ([`Observed::at`]); hashed, never kept as sent. The label is the
    /// harness the header or the user agent names.
    pub(crate) fn read<'h>(header: impl Fn(&str) -> Option<&'h str>) -> Self {
        let user_agent = header("user-agent").map(printable);
        let product = user_agent
            .as_deref()
            .map(|agent| {
                agent
                    .split(['/', ' '])
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .filter(|product| !product.is_empty());
        let label = match product.as_deref() {
            Some("claude-cli") => "claude-code".to_owned(),
            Some(product) if product.starts_with("codex") => "codex".to_owned(),
            Some(product) => product.chars().take(32).collect(),
            None => "unknown".to_owned(),
        };
        let client_retry = header("x-stainless-retry-count").and_then(|count| count.parse().ok());
        let named = if let Some(id) = header("x-claude-code-session-id") {
            Some(("claude-code", id, "claude-code".to_owned()))
        } else {
            header("session-id")
                .or_else(|| header("session_id"))
                .or_else(|| header(crate::relay::CALLER_SESSION_HEADER))
                .map(|id| {
                    let label = if label == "unknown" {
                        "codex".to_owned()
                    } else {
                        label.clone()
                    };
                    ("session-id", id, label)
                })
        };
        match named {
            Some((source, id, label)) => Self {
                session: session_hash(source, id),
                label,
                user_agent,
                client_retry,
                by_address: false,
            },
            None => Self {
                session: String::new(),
                label,
                user_agent,
                client_retry,
                by_address: true,
            }
            .at(None),
        }
    }

    /// The session keyed by user agent and `peer`, when no header named it.
    pub(crate) fn at(mut self, peer: Option<IpAddr>) -> Self {
        if self.by_address {
            let peer = peer.map(|peer| peer.to_string()).unwrap_or_default();
            let agent = self.user_agent.as_deref().unwrap_or_default();
            self.session = session_hash("agent-address", &format!("{agent}\0{peer}"));
        }
        self
    }
}

/// The first [`SESSION_HEX`] hex digits of a SHA-256 over the source and value.
fn session_hash(source: &str, value: &str) -> String {
    let digest =
        crate::auth::sha256(format!("llm-gateway.session/1\0{source}\0{value}").as_bytes());
    let mut session = String::with_capacity(SESSION_HEX);
    for byte in &digest[..SESSION_HEX / 2] {
        let _ = write!(session, "{byte:02x}");
    }
    session
}

/// `value` as printable ASCII of at most [`MAX_USER_AGENT_BYTES`], every other byte a `?`.
fn printable(value: &str) -> String {
    value
        .bytes()
        .take(MAX_USER_AGENT_BYTES)
        .map(|byte| {
            if (0x20..0x7f).contains(&byte) {
                char::from(byte)
            } else {
                '?'
            }
        })
        .collect()
}

/// One finished call, as the server hands it to [`Dashboard::finish`].
#[derive(Debug, Clone)]
pub(crate) struct Finished {
    pub(crate) id: u64,
    pub(crate) started_ms: u64,
    pub(crate) upstream: Option<String>,
    /// The model name the target was sent: an alias resolved to its newest model.
    pub(crate) upstream_model: Option<String>,
    pub(crate) queue_wait_ms: Option<u64>,
    pub(crate) ttft_ms: Option<u64>,
    pub(crate) usage: Usage,
    pub(crate) body_digest: Option<[u8; 32]>,
    pub(crate) client_disconnected: bool,
    pub(crate) upstream_retry_after: Option<String>,
    /// Each attempt at the target, in order; empty for a call no passthrough route served.
    pub(crate) attempts: Vec<Attempt>,
    pub(crate) observed: Observed,
    /// The label of the credential the client sent to a passthrough route
    /// ([`kpi::credential_label`]); never the credential.
    pub(crate) credential: Option<String>,
    /// Why the usage is unknown though the answer carried it, e.g. `undecodable(br)`.
    pub(crate) usage_note: Option<String>,
}

/// One attempt of a passthrough call at its target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Attempt {
    /// 1 for the first attempt.
    pub(crate) number: u32,
    /// The status the target answered with; `None` when it gave no answer head.
    pub(crate) status: Option<u16>,
    pub(crate) duration_ms: u64,
    /// How long the gateway waited after this attempt before the next; 0 for the last.
    pub(crate) wait_ms: u64,
    pub(crate) retry_after: Option<String>,
    /// The answer lines a client reads to retry and to show its limits: `retry-after`,
    /// `x-should-retry`, `request-id` and every `anthropic-ratelimit-*`.
    pub(crate) headers: Vec<(String, String)>,
    /// The start of a non-2xx answer's body, token-like strings redacted.
    pub(crate) error_body: Option<String>,
}

#[derive(Debug, Clone)]
struct CallRow {
    id: u64,
    started_ms: u64,
    route: Option<String>,
    upstream: Option<String>,
    upstream_model: Option<String>,
    wire: Wire,
    status: u16,
    target_status: Option<u16>,
    outcome: Outcome,
    refusal: Option<&'static str>,
    queue_wait_ms: Option<u64>,
    ttft_ms: Option<u64>,
    duration_ms: u64,
    response_bytes: u64,
    usage: Usage,
    client_retry: Option<u64>,
    repeated_body: bool,
    client_disconnected: bool,
    upstream_retry_after: Option<String>,
    attempts: Vec<Attempt>,
    session: String,
    user_agent: Option<String>,
    usage_note: Option<String>,
}

impl CallRow {
    fn input_tps(&self) -> Option<f64> {
        input_sample(&self.usage, self.wire, self.ttft_ms).and_then(|(tokens, ms)| rate(tokens, ms))
    }

    fn output_tps(&self) -> Option<f64> {
        output_sample(&self.usage, self.ttft_ms, self.duration_ms)
            .and_then(|(tokens, ms)| rate(tokens, ms))
    }
}

/// The fewest output tokens, and the shortest time after the first token, an output rate is
/// read from: a short answer arrives in a burst or two, and its rate measures the burst.
pub const MIN_RATED_OUTPUT_TOKENS: u64 = 50;
pub const MIN_RATED_STREAMING_MS: u64 = 250;

/// Every input token a call's model read: on the messages wire `input_tokens` leaves out the
/// cache reads and writes, which the other wires count in.
fn total_input(usage: &Usage, wire: Wire) -> Option<u64> {
    let input = usage.input_tokens?;
    Some(if wire == Wire::Messages {
        input + usage.cache_read_tokens.unwrap_or(0) + usage.cache_creation_tokens.unwrap_or(0)
    } else {
        input
    })
}

/// The input tokens and the time to first token an input rate is read from.
fn input_sample(usage: &Usage, wire: Wire, ttft_ms: Option<u64>) -> Option<(u64, u64)> {
    let ttft = ttft_ms.filter(|ttft| *ttft > 0)?;
    Some((total_input(usage, wire)?, ttft))
}

/// The output tokens and the time after the first token an output rate is read from; none
/// below [`MIN_RATED_OUTPUT_TOKENS`] or [`MIN_RATED_STREAMING_MS`].
fn output_sample(usage: &Usage, ttft_ms: Option<u64>, duration_ms: u64) -> Option<(u64, u64)> {
    let tokens = usage.output_tokens?;
    let streaming = duration_ms.saturating_sub(ttft_ms?);
    (tokens >= MIN_RATED_OUTPUT_TOKENS && streaming >= MIN_RATED_STREAMING_MS)
        .then_some((tokens, streaming))
}

#[derive(Debug, Clone)]
struct Flight {
    started_ms: u64,
    route: Option<String>,
    wire: Wire,
    streaming: bool,
    attempt: u32,
    retry_at_ms: Option<u64>,
    session: String,
}

#[derive(Debug, Clone, Default)]
struct SessionTotals {
    label: String,
    calls: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    /// Input tokens of the calls with a time to first token, and the sum of those times.
    rate_input_tokens: u64,
    ttft_ms: u64,
    /// Output tokens of the calls with a streaming time, and the sum of those times.
    rate_output_tokens: u64,
    streaming_ms: u64,
    last_ms: u64,
}

impl SessionTotals {
    fn input_tps(&self) -> Option<f64> {
        rate(self.rate_input_tokens, self.ttft_ms)
    }

    fn output_tps(&self) -> Option<f64> {
        rate(self.rate_output_tokens, self.streaming_ms)
    }
}

#[allow(clippy::cast_precision_loss)]
fn rate(tokens: u64, milliseconds: u64) -> Option<f64> {
    (milliseconds > 0).then(|| tokens as f64 * 1000.0 / milliseconds as f64)
}

/// The histogram bounds of the time-to-first-token and queue-wait series, in milliseconds.
const BUCKETS_MS: [u64; 12] = [
    50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 30_000, 60_000, 120_000, 300_000,
];

#[derive(Debug, Clone, Copy, Default)]
struct Histogram {
    buckets: [u64; 12],
    sum_ms: u64,
    count: u64,
}

impl Histogram {
    fn observe(&mut self, milliseconds: u64) {
        for (bucket, bound) in self.buckets.iter_mut().zip(BUCKETS_MS) {
            if milliseconds <= bound {
                *bucket = bucket.saturating_add(1);
            }
        }
        self.sum_ms = self.sum_ms.saturating_add(milliseconds);
        self.count = self.count.saturating_add(1);
    }
}

#[derive(Debug, Clone, Default)]
struct RouteTotals {
    calls: u64,
    errors: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    /// Input tokens no cache served ([`kpi::uncached`]).
    uncached_input_tokens: u64,
    /// The calls' tokens per second, and the sums behind the route's averages.
    input_tps: kpi::TpsHistogram,
    output_tps: kpi::TpsHistogram,
    client_retries: u64,
    repeated_bodies: u64,
    client_disconnects: u64,
    /// Answers with a 429 or 5xx status from the target, by status.
    upstream_statuses: BTreeMap<u16, u64>,
    ttft: Histogram,
    queue_wait: Histogram,
    /// Attempts at the target, by status (`error` for one without an answer head).
    attempts: BTreeMap<String, u64>,
    retry_wait: Histogram,
    last_call_end_ms: Option<u64>,
    upstream_model: Option<String>,
}

impl RouteTotals {
    /// Adds one finished call of this route.
    fn add(&mut self, record: &UsageRecord, finished: &Finished, repeated_body: bool, now: u64) {
        let usage = &finished.usage;
        let errored = record.outcome != Outcome::Ok || record.status >= 400;
        self.calls += 1;
        self.errors += u64::from(errored);
        self.input_tokens += usage.input_tokens.unwrap_or(0);
        self.output_tokens += usage.output_tokens.unwrap_or(0);
        self.cache_read_tokens += usage.cache_read_tokens.unwrap_or(0);
        self.cache_creation_tokens += usage.cache_creation_tokens.unwrap_or(0);
        self.uncached_input_tokens += kpi::uncached(
            usage.input_tokens,
            usage.cache_read_tokens,
            record.wire == Wire::Messages,
        );
        if let Some(tps) = input_sample(usage, record.wire, finished.ttft_ms)
            .and_then(|(tokens, ms)| rate(tokens, ms))
        {
            self.input_tps.observe(tps);
        }
        if let Some(tps) = output_sample(usage, finished.ttft_ms, record.duration_ms)
            .and_then(|(tokens, ms)| rate(tokens, ms))
        {
            self.output_tps.observe(tps);
        }
        self.client_retries += u64::from(finished.observed.client_retry.unwrap_or(0) > 0);
        self.repeated_bodies += u64::from(repeated_body);
        self.client_disconnects += u64::from(finished.client_disconnected);
        count_attempts(self, &finished.attempts, record.target_status);
        if let Some(ttft) = finished.ttft_ms {
            self.ttft.observe(ttft);
        }
        if let Some(wait) = finished.queue_wait_ms {
            self.queue_wait.observe(wait);
        }
        self.last_call_end_ms = Some(now);
        if finished.upstream_model.is_some() {
            self.upstream_model.clone_from(&finished.upstream_model);
        }
    }
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    calls: VecDeque<CallRow>,
    in_flight: BTreeMap<u64, Flight>,
    sessions: BTreeMap<String, SessionTotals>,
    routes: BTreeMap<String, RouteTotals>,
    digests: VecDeque<([u8; 32], u64)>,
    provider: Option<ProviderPanel>,
    /// The subscription limits last reported, by credential label.
    limits: BTreeMap<String, kpi::LimitEntry>,
    /// Watchdog limits for the shutdown countdown, set by the embedding.
    watchdog: Option<WatchdogLimits>,
    /// Each route's upstream server, as the embedding last read it.
    upstreams: BTreeMap<String, UpstreamStats>,
    /// Raised on every change, so an event stream knows when to write.
    generation: u64,
}

/// The dashboard's state, shared by every connection of one gateway.
#[derive(Debug)]
pub struct Dashboard {
    started_ms: u64,
    state: Mutex<State>,
    changed: Condvar,
}

impl Default for Dashboard {
    fn default() -> Self {
        Self {
            started_ms: now_ms(),
            state: Mutex::default(),
            changed: Condvar::new(),
        }
    }
}

/// A call in flight, removed from the registry when dropped.
pub(crate) struct Progress {
    dashboard: Arc<Dashboard>,
    id: u64,
}

impl std::fmt::Debug for Progress {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Progress")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Progress {
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    /// The relayed model the call is for, once known.
    pub(crate) fn route(&self, route: &str) {
        self.dashboard.update(self.id, |flight| {
            flight.route = Some(route.to_owned());
        });
    }

    /// The first byte of the answer was written to the client.
    pub(crate) fn streaming(&self) {
        self.dashboard
            .update(self.id, |flight| flight.streaming = true);
    }

    /// The call waits until `retry_at_ms` before its attempt `attempt`; `None` once it is
    /// under way.
    pub(crate) fn retrying(&self, attempt: u32, retry_at_ms: Option<u64>) {
        self.dashboard.update(self.id, |flight| {
            flight.attempt = attempt;
            flight.retry_at_ms = retry_at_ms;
        });
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let mut state = self.dashboard.lock();
        if state.in_flight.remove(&self.id).is_some() {
            state.generation = state.generation.wrapping_add(1);
            drop(state);
            self.dashboard.changed.notify_all();
        }
    }
}

impl Dashboard {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn changed(&self, mut state: std::sync::MutexGuard<'_, State>) {
        state.generation = state.generation.wrapping_add(1);
        drop(state);
        self.changed.notify_all();
    }

    fn update(&self, id: u64, change: impl FnOnce(&mut Flight)) {
        let mut state = self.lock();
        if let Some(flight) = state.in_flight.get_mut(&id) {
            change(flight);
            self.changed(state);
        }
    }

    /// Registers a call that is starting now, in flight until the [`Progress`] is dropped.
    pub(crate) fn begin(self: &Arc<Self>, wire: Wire, observed: &Observed) -> Progress {
        let mut state = self.lock();
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        state.in_flight.insert(
            id,
            Flight {
                started_ms: now_ms(),
                route: None,
                wire,
                streaming: false,
                attempt: 1,
                retry_at_ms: None,
                session: observed.session.clone(),
            },
        );
        self.changed(state);
        Progress {
            dashboard: Arc::clone(self),
            id,
        }
    }

    /// Records a finished call under its usage record.
    pub(crate) fn finish(&self, record: &UsageRecord, finished: Finished) {
        let now = now_ms();
        let mut state = self.lock();
        // The repeated-body flag: the same body digest within REPEAT_WINDOW.
        while state
            .digests
            .front()
            .is_some_and(|(_, at)| now.saturating_sub(*at) > window_ms())
            || state.digests.len() > DIGESTS
        {
            state.digests.pop_front();
        }
        let repeated_body = finished.body_digest.is_some_and(|digest| {
            let seen = state.digests.iter().any(|(known, _)| *known == digest);
            state.digests.push_back((digest, now));
            seen
        });
        let usage = finished.usage;
        let session = state
            .sessions
            .entry(finished.observed.session.clone())
            .or_default();
        session.label.clone_from(&finished.observed.label);
        session.calls += 1;
        session.input_tokens += usage.input_tokens.unwrap_or(0);
        session.output_tokens += usage.output_tokens.unwrap_or(0);
        session.cache_read_tokens += usage.cache_read_tokens.unwrap_or(0);
        if let Some((tokens, ttft)) = input_sample(&usage, record.wire, finished.ttft_ms) {
            session.rate_input_tokens += tokens;
            session.ttft_ms += ttft;
        }
        if let Some((tokens, streaming)) =
            output_sample(&usage, finished.ttft_ms, record.duration_ms)
        {
            session.rate_output_tokens += tokens;
            session.streaming_ms += streaming;
        }
        session.last_ms = now;
        if state.sessions.len() > MAX_SESSIONS
            && let Some(oldest) = state
                .sessions
                .iter()
                .min_by_key(|(_, totals)| totals.last_ms)
                .map(|(key, _)| key.clone())
        {
            state.sessions.remove(&oldest);
        }
        if let Some(model) = &record.model {
            let route = state.routes.entry(model.clone()).or_default();
            route.add(record, &finished, repeated_body, now);
        }
        kpi::note_limits(
            &mut state.limits,
            finished.credential.as_deref(),
            &finished.attempts,
            now,
        );
        state.calls.push_front(CallRow {
            id: finished.id,
            started_ms: finished.started_ms,
            route: record.model.clone(),
            upstream: finished.upstream,
            upstream_model: finished.upstream_model.clone(),
            wire: record.wire,
            status: record.status,
            target_status: record.target_status,
            outcome: record.outcome,
            refusal: record.refusal.map(RefusalCode::wire),
            queue_wait_ms: finished.queue_wait_ms,
            ttft_ms: finished.ttft_ms,
            duration_ms: record.duration_ms,
            response_bytes: record.response_bytes,
            usage,
            client_retry: finished.observed.client_retry,
            repeated_body,
            client_disconnected: finished.client_disconnected,
            upstream_retry_after: finished.upstream_retry_after,
            attempts: finished.attempts,
            session: finished.observed.session,
            user_agent: finished.observed.user_agent,
            usage_note: finished.usage_note,
        });
        state.calls.truncate(MAX_CALLS);
        self.changed(state);
    }

    /// Sets the watchdog limits the pods' shutdown countdowns are read from.
    pub fn set_watchdog(&self, limits: Option<WatchdogLimits>) {
        let mut state = self.lock();
        state.watchdog = limits;
        self.changed(state);
    }

    /// Replaces what `route`'s upstream server last said about itself; `None` removes it.
    pub fn set_upstream(&self, route: &str, read: Option<UpstreamStats>) {
        let mut state = self.lock();
        match read {
            Some(read) => {
                state.upstreams.insert(route.to_owned(), read);
            }
            None => {
                state.upstreams.remove(route);
            }
        }
        self.changed(state);
    }

    /// Replaces the provider panel; `None` removes it.
    pub fn set_provider(&self, provider: Option<ProviderPanel>) {
        let mut state = self.lock();
        state.provider = provider;
        self.changed(state);
    }

    /// Per relayed model that has had a call or has one in flight: when its last call
    /// finished and how many are in flight, for the embedding's heartbeat.
    pub fn route_activity(&self) -> Vec<RouteActivity> {
        let state = self.lock();
        let mut activity: BTreeMap<String, RouteActivity> = state
            .routes
            .iter()
            .map(|(route, totals)| {
                (
                    route.clone(),
                    RouteActivity {
                        route: route.clone(),
                        last_call_end_ms: totals.last_call_end_ms,
                        in_flight: 0,
                    },
                )
            })
            .collect();
        for flight in state.in_flight.values() {
            if let Some(route) = &flight.route {
                activity
                    .entry(route.clone())
                    .or_insert_with(|| RouteActivity {
                        route: route.clone(),
                        last_call_end_ms: None,
                        in_flight: 0,
                    })
                    .in_flight += 1;
            }
        }
        activity.into_values().collect()
    }

    /// The change counter an event stream waits on.
    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Waits until the state changes from `seen` or `timeout` passes, and returns the counter.
    pub(crate) fn wait_for_change(&self, seen: u64, timeout: Duration) -> u64 {
        let state = self.lock();
        let (state, _) = self
            .changed
            .wait_timeout_while(state, timeout, |state| state.generation == seen)
            .unwrap_or_else(PoisonError::into_inner);
        state.generation
    }

    /// The snapshot as JSON (`llm-gateway.dashboard/1`).
    pub fn snapshot(&self) -> String {
        let now = now_ms();
        let state = self.lock();
        let mut writer = Writer::new();
        record(&mut writer, &SNAPSHOT, |writer, key| match key {
            "format" => writer.string(SNAPSHOT_FORMAT),
            "now_ms" => writer.number(now),
            "started_ms" => writer.number(self.started_ms),
            "calls" => {
                writer.begin_array();
                for call in &state.calls {
                    call_record(writer, call);
                }
                writer.end_array();
            }
            "in_flight" => {
                writer.begin_array();
                for (id, flight) in &state.in_flight {
                    flight_record(writer, *id, flight, now);
                }
                writer.end_array();
            }
            "sessions" => {
                writer.begin_array();
                let mut sessions: Vec<_> = state.sessions.iter().collect();
                sessions.sort_by_key(|(_, totals)| std::cmp::Reverse(totals.last_ms));
                for (key, totals) in sessions {
                    session_record(writer, key, totals);
                }
                writer.end_array();
            }
            "routes" => {
                writer.begin_array();
                for (route, totals) in &state.routes {
                    route_record(
                        writer,
                        route,
                        totals,
                        in_flight(&state, route),
                        state.upstreams.get(route),
                    );
                }
                writer.end_array();
            }
            "provider" => match &state.provider {
                Some(provider) => provider_record(writer, provider, &state, now),
                None => writer.null(),
            },
            "kpi" => kpi::kpi_record(writer, &state, now),
            "limits" => kpi::limits_record(writer, &state),
            _ => writer.null(),
        });
        writer.finish()
    }

    /// The dashboard's Prometheus series, appended to `GET /metrics`.
    pub fn render_prometheus(&self) -> String {
        let now = now_ms();
        let state = self.lock();
        let mut text = String::new();
        let out = &mut text;
        render_tokens(out, &state);
        render_histograms(out, &state);
        kpi::render(out, &state, now);
        upstream::render(out, &state);
        let in_flight = series(
            out,
            "llmgw_in_flight_requests",
            "gauge",
            "Model calls in flight now.",
        );
        let _ = writeln!(out, "{in_flight} {}", state.in_flight.len());
        let counters: [(&str, &str, Pick<u64>); 3] = [
            (
                "llmgw_client_retries_total",
                "Calls whose client said it was retrying (x-stainless-retry-count above 0), per model.",
                |totals| totals.client_retries,
            ),
            (
                "llmgw_repeated_bodies_total",
                "Calls whose body repeated one seen in the last ten minutes, per model.",
                |totals| totals.repeated_bodies,
            ),
            (
                "llmgw_client_disconnects_total",
                "Calls whose client went away while the answer was written, per model.",
                |totals| totals.client_disconnects,
            ),
        ];
        for (name, help, pick) in counters {
            let name = series(out, name, "counter", help);
            for (route, totals) in &state.routes {
                line(
                    out,
                    &name,
                    &format!("model=\"{}\"", escape(route)),
                    pick(totals),
                );
            }
        }
        let statuses = series(
            out,
            "llmgw_upstream_status_total",
            "counter",
            "Answers the target gave with a 429 or 5xx status, per model and status.",
        );
        for (route, totals) in &state.routes {
            let route = escape(route);
            for (status, count) in &totals.upstream_statuses {
                line(
                    out,
                    &statuses,
                    &format!("model=\"{route}\",status=\"{status}\""),
                    *count,
                );
            }
        }
        let attempts = series(
            out,
            "llmgw_upstream_attempts_total",
            "counter",
            "Attempts at the target, per model and the status it answered (error: no answer head).",
        );
        for (route, totals) in &state.routes {
            let route = escape(route);
            for (status, count) in &totals.attempts {
                line(
                    out,
                    &attempts,
                    &format!("model=\"{route}\",status=\"{status}\""),
                    *count,
                );
            }
        }
        render_sessions(out, &state);
        if let Some(provider) = &state.provider {
            render_provider(out, provider, now);
            render_shutdowns(out, &state, provider, now);
        }
        text
    }
}

/// Per owned pod, the seconds until the watchdog stops it and the limit that fires first.
fn render_shutdowns(out: &mut String, state: &State, provider: &ProviderPanel, now: u64) {
    let Some(limits) = state.watchdog else {
        return;
    };
    let name = series(
        out,
        "llmgw_pod_shutdown_seconds",
        "gauge",
        "Seconds until the watchdog stops an owned pod, labelled with the limit that fires first.",
    );
    for pod in &provider.pods {
        if pod.external {
            continue;
        }
        let model = pod.model.as_deref();
        let last_call = model
            .and_then(|model| state.routes.get(model))
            .and_then(|route| route.last_call_end_ms);
        let flights = model.map_or(0, |model| in_flight(state, model));
        if let Some((at, limit)) = shutdown(&limits, provider, pod, last_call, flights) {
            let _ = writeln!(
                out,
                "{name}{{pod=\"{}\",limit=\"{limit}\"}} {}",
                escape(&pod.id),
                seconds(at.saturating_sub(now))
            );
        }
    }
}

/// What one per-model series reads from a route's totals.
type Pick<T> = fn(&RouteTotals) -> T;

/// The token and cache-token counters, per model.
fn render_tokens(out: &mut String, state: &State) {
    {
        let tokens = series(
            out,
            "llmgw_tokens_total",
            "counter",
            "Tokens the answers reported, per model and direction (input, output).",
        );
        for (route, totals) in &state.routes {
            let route = escape(route);
            line(
                out,
                &tokens,
                &format!("model=\"{route}\",direction=\"input\""),
                totals.input_tokens,
            );
            line(
                out,
                &tokens,
                &format!("model=\"{route}\",direction=\"output\""),
                totals.output_tokens,
            );
        }
        let cache = series(
            out,
            "llmgw_cache_tokens_total",
            "counter",
            "Prompt-cache tokens the answers reported, per model and kind (read, creation).",
        );
        for (route, totals) in &state.routes {
            let route = escape(route);
            line(
                out,
                &cache,
                &format!("model=\"{route}\",kind=\"read\""),
                totals.cache_read_tokens,
            );
            line(
                out,
                &cache,
                &format!("model=\"{route}\",kind=\"creation\""),
                totals.cache_creation_tokens,
            );
        }
    }
}

/// The time-to-first-token and queue-wait histograms, per model.
fn render_histograms(out: &mut String, state: &State) {
    {
        let histograms: [(&str, &str, Pick<Histogram>); 3] = [
            (
                "llmgw_time_to_first_token_seconds",
                "Seconds from the end of a relayed call's request head to the first answer byte written, per model.",
                |totals| totals.ttft,
            ),
            (
                "llmgw_queue_wait_seconds",
                "Seconds from the end of a relayed call's request head until its target was connected, per model.",
                |totals| totals.queue_wait,
            ),
            (
                "llmgw_upstream_retry_wait_seconds",
                "Seconds a passthrough call waited before retrying its target, one sample per wait, per model.",
                |totals| totals.retry_wait,
            ),
        ];
        for (name, help, pick) in histograms {
            let name = series(out, name, "histogram", help);
            for (route, totals) in &state.routes {
                let route = escape(route);
                let histogram = pick(totals);
                for (bound, count) in BUCKETS_MS.iter().zip(histogram.buckets) {
                    let bound = seconds(*bound);
                    let _ = writeln!(
                        out,
                        "{name}_bucket{{model=\"{route}\",le=\"{bound}\"}} {count}"
                    );
                }
                let count = histogram.count;
                let _ = writeln!(
                    out,
                    "{name}_bucket{{model=\"{route}\",le=\"+Inf\"}} {count}"
                );
                let sum = seconds(histogram.sum_ms);
                let _ = writeln!(out, "{name}_sum{{model=\"{route}\"}} {sum}");
                let _ = writeln!(out, "{name}_count{{model=\"{route}\"}} {count}");
            }
        }
    }
}

/// Counts a call's 429 or 5xx answer, its attempts at its target by status, and its retry waits;
/// a call no passthrough route served counts its one answer as its attempt.
fn count_attempts(route: &mut RouteTotals, attempts: &[Attempt], target_status: Option<u16>) {
    if let Some(status) = target_status.filter(|status| *status == 429 || *status >= 500) {
        *route.upstream_statuses.entry(status).or_default() += 1;
    }
    if attempts.is_empty()
        && let Some(status) = target_status
    {
        *route.attempts.entry(status.to_string()).or_default() += 1;
    }
    for attempt in attempts {
        let status = attempt
            .status
            .map_or_else(|| "error".to_owned(), |status| status.to_string());
        *route.attempts.entry(status).or_default() += 1;
        if attempt.wait_ms > 0 {
            route.retry_wait.observe(attempt.wait_ms);
        }
    }
}

/// Writes a series' `HELP` and `TYPE` lines and returns its name.
fn series(out: &mut String, name: &str, kind: &str, help: &str) -> String {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    name.to_owned()
}

/// Writes one sample of `name` with `labels`.
fn line(out: &mut String, name: &str, labels: &str, value: u64) {
    let _ = writeln!(out, "{name}{{{labels}}} {value}");
}

/// The per-session series: calls, and the input and output token rates.
fn render_sessions(out: &mut String, state: &State) {
    let calls = series(
        out,
        "llmgw_session_calls_total",
        "counter",
        "Calls per session (a hash of the harness's session header, or of user agent and client address).",
    );
    for (key, totals) in &state.sessions {
        let label = escape(&totals.label);
        line(
            out,
            &calls,
            &format!("session=\"{key}\",label=\"{label}\""),
            totals.calls,
        );
    }
    let rates = series(
        out,
        "llmgw_session_tokens_per_second",
        "gauge",
        "Per session, input tokens over time to first token and output tokens over streaming time after it.",
    );
    for (key, totals) in &state.sessions {
        let label = escape(&totals.label);
        for (direction, value) in [
            ("input", totals.input_tps()),
            ("output", totals.output_tps()),
        ] {
            if let Some(value) = value {
                let _ = writeln!(
                    out,
                    "{rates}{{session=\"{key}\",label=\"{label}\",direction=\"{direction}\"}} {value:.3}"
                );
            }
        }
    }
}

/// The provider panel's series: balance, spend, owned pods and the watchdog's heartbeat age.
fn render_provider(out: &mut String, provider: &ProviderPanel, now: u64) {
    let gauges = [
        (
            "llmgw_runpod_balance_usd",
            "The provider account's balance in USD, as last read.",
            provider.balance_usd,
        ),
        (
            "llmgw_runpod_spend_per_hour_usd",
            "The provider account's spend per hour in USD, as last read.",
            provider.spend_per_hour_usd,
        ),
    ];
    for (name, help, value) in gauges {
        if let Some(value) = value.filter(|value| value.is_finite()) {
            let name = series(out, name, "gauge", help);
            let _ = writeln!(out, "{name} {value:.4}");
        }
    }
    let owned = series(
        out,
        "llmgw_runpod_owned_pods",
        "gauge",
        "Pods the gateway owns, as last read.",
    );
    let _ = writeln!(out, "{owned} {}", provider.pods.len());
    let other = series(
        out,
        "llmgw_runpod_other_pods",
        "gauge",
        "The provider account's pods the gateway does not own, as last read.",
    );
    let _ = writeln!(out, "{other} {}", provider.other_pods);
    if let Some(at) = provider.watchdog_heartbeat_ms {
        let age = series(
            out,
            "llmgw_watchdog_heartbeat_age_seconds",
            "gauge",
            "Seconds since the pod watchdog last wrote its heartbeat.",
        );
        let _ = writeln!(out, "{age} {}", seconds(now.saturating_sub(at)));
    }
}

fn window_ms() -> u64 {
    u64::try_from(REPEAT_WINDOW.as_millis()).unwrap_or(u64::MAX)
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn seconds(milliseconds: u64) -> String {
    let (whole, fraction) = (milliseconds / 1000, milliseconds % 1000);
    if fraction == 0 {
        return whole.to_string();
    }
    let digits = format!("{fraction:03}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

fn in_flight(state: &State, route: &str) -> u64 {
    let count = state
        .in_flight
        .values()
        .filter(|flight| flight.route.as_deref() == Some(route))
        .count();
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Writes one object of `kind`, each field in its declared order through `value`.
fn record(writer: &mut Writer, kind: &RecordType, mut value: impl FnMut(&mut Writer, &str)) {
    writer.begin_object();
    for (key, _) in kind.fields {
        writer.key(key);
        value(writer, key);
    }
    writer.end_object();
}

fn optional_number(writer: &mut Writer, value: Option<u64>) {
    match value {
        Some(value) => writer.number(value),
        None => writer.null(),
    }
}

fn optional_string(writer: &mut Writer, value: Option<&str>) {
    match value {
        Some(value) => writer.string(value),
        None => writer.null(),
    }
}

fn optional_decimal(writer: &mut Writer, value: Option<f64>) {
    match value {
        Some(value) => writer.decimal(value),
        None => writer.null(),
    }
}

fn call_record(writer: &mut Writer, call: &CallRow) {
    record(writer, &CALL, |writer, key| match key {
        "id" => writer.number(call.id),
        "started_ms" => writer.number(call.started_ms),
        "route" => optional_string(writer, call.route.as_deref()),
        "upstream" => optional_string(writer, call.upstream.as_deref()),
        "upstream_model" => optional_string(writer, call.upstream_model.as_deref()),
        "wire" => writer.string(call.wire.label()),
        "status" => writer.number(u64::from(call.status)),
        "target_status" => optional_number(writer, call.target_status.map(u64::from)),
        "outcome" => writer.string(call.outcome.label()),
        "refusal" => optional_string(writer, call.refusal),
        "queue_wait_ms" => optional_number(writer, call.queue_wait_ms),
        "ttft_ms" => optional_number(writer, call.ttft_ms),
        "duration_ms" => writer.number(call.duration_ms),
        "response_bytes" => writer.number(call.response_bytes),
        "input_tokens" => optional_number(writer, call.usage.input_tokens),
        "output_tokens" => optional_number(writer, call.usage.output_tokens),
        "cache_read_tokens" => optional_number(writer, call.usage.cache_read_tokens),
        "cache_creation_tokens" => optional_number(writer, call.usage.cache_creation_tokens),
        "input_tps" => optional_decimal(writer, call.input_tps()),
        "output_tps" => optional_decimal(writer, call.output_tps()),
        "usage_note" => optional_string(writer, call.usage_note.as_deref()),
        "client_retry" => optional_number(writer, call.client_retry),
        "repeated_body" => writer.boolean(call.repeated_body),
        "client_disconnected" => writer.boolean(call.client_disconnected),
        "upstream_retry_after" => optional_string(writer, call.upstream_retry_after.as_deref()),
        "attempts" => {
            writer.begin_array();
            for attempt in &call.attempts {
                attempt_record(writer, attempt);
            }
            writer.end_array();
        }
        "session" => writer.string(&call.session),
        "user_agent" => optional_string(writer, call.user_agent.as_deref()),
        _ => writer.null(),
    });
}

fn attempt_record(writer: &mut Writer, attempt: &Attempt) {
    record(writer, &ATTEMPT, |writer, key| match key {
        "attempt" => writer.number(u64::from(attempt.number)),
        "status" => optional_number(writer, attempt.status.map(u64::from)),
        "duration_ms" => writer.number(attempt.duration_ms),
        "wait_ms" => writer.number(attempt.wait_ms),
        "retry_after" => optional_string(writer, attempt.retry_after.as_deref()),
        "headers" => {
            writer.begin_object();
            for (name, value) in &attempt.headers {
                writer.key(name);
                writer.string(value);
            }
            writer.end_object();
        }
        "error_body" => optional_string(writer, attempt.error_body.as_deref()),
        _ => writer.null(),
    });
}

fn flight_record(writer: &mut Writer, id: u64, flight: &Flight, now: u64) {
    record(writer, &IN_FLIGHT, |writer, key| match key {
        "id" => writer.number(id),
        "started_ms" => writer.number(flight.started_ms),
        "route" => optional_string(writer, flight.route.as_deref()),
        "wire" => writer.string(flight.wire.label()),
        "state" => writer.string(if flight.streaming {
            "streaming"
        } else if flight.retry_at_ms.is_some() {
            "waiting-retry"
        } else {
            "waiting-upstream"
        }),
        "attempt" => writer.number(u64::from(flight.attempt)),
        "retry_at_ms" => optional_number(writer, flight.retry_at_ms),
        "elapsed_ms" => writer.number(now.saturating_sub(flight.started_ms)),
        "session" => writer.string(&flight.session),
        _ => writer.null(),
    });
}

fn session_record(writer: &mut Writer, key: &str, totals: &SessionTotals) {
    record(writer, &SESSION, |writer, field| match field {
        "session" => writer.string(key),
        "label" => writer.string(&totals.label),
        "calls" => writer.number(totals.calls),
        "input_tokens" => writer.number(totals.input_tokens),
        "output_tokens" => writer.number(totals.output_tokens),
        "cache_read_tokens" => writer.number(totals.cache_read_tokens),
        "input_tps" => optional_decimal(writer, totals.input_tps()),
        "output_tps" => optional_decimal(writer, totals.output_tps()),
        "last_ms" => writer.number(totals.last_ms),
        _ => writer.null(),
    });
}

fn route_record(
    writer: &mut Writer,
    route: &str,
    totals: &RouteTotals,
    in_flight: u64,
    upstream: Option<&UpstreamStats>,
) {
    record(writer, &ROUTE, |writer, key| match key {
        "route" => writer.string(route),
        "calls" => writer.number(totals.calls),
        "errors" => writer.number(totals.errors),
        "input_tokens" => writer.number(totals.input_tokens),
        "output_tokens" => writer.number(totals.output_tokens),
        "cache_read_tokens" => writer.number(totals.cache_read_tokens),
        "cache_creation_tokens" => writer.number(totals.cache_creation_tokens),
        "uncached_input_tokens" => writer.number(totals.uncached_input_tokens),
        "input_tps" => optional_decimal(writer, totals.input_tps.mean()),
        "output_tps" => optional_decimal(writer, totals.output_tps.mean()),
        "in_flight" => writer.number(in_flight),
        "upstream_model" => optional_string(writer, totals.upstream_model.as_deref()),
        "upstream" => upstream::record(writer, upstream),
        _ => writer.null(),
    });
}

fn provider_record(writer: &mut Writer, provider: &ProviderPanel, state: &State, now: u64) {
    record(writer, &PROVIDER, |writer, key| match key {
        "name" => writer.string(&provider.name),
        "read_ms" => writer.number(provider.read_ms),
        "balance_usd" => optional_decimal(writer, provider.balance_usd),
        "spend_per_hour_usd" => optional_decimal(writer, provider.spend_per_hour_usd),
        "other_pods" => writer.number(provider.other_pods),
        "pods" => {
            writer.begin_array();
            for pod in &provider.pods {
                let route = pod
                    .model
                    .as_deref()
                    .and_then(|model| state.routes.get(model));
                let flights = pod
                    .model
                    .as_deref()
                    .map_or(0, |model| in_flight(state, model));
                let last_call = route.and_then(|route| route.last_call_end_ms);
                let shutdown = state
                    .watchdog
                    .filter(|_| !pod.external)
                    .and_then(|limits| shutdown(&limits, provider, pod, last_call, flights));
                let ready = if pod.external {
                    pod.model
                        .as_deref()
                        .and_then(|model| state.upstreams.get(model))
                        .and_then(|up| up.ready)
                } else {
                    pod.vllm_ready
                };
                pod_record(writer, pod, ready, last_call, flights, shutdown);
            }
            writer.end_array();
        }
        "watchdog_heartbeat_age_ms" => optional_number(
            writer,
            provider
                .watchdog_heartbeat_ms
                .map(|at| now.saturating_sub(at)),
        ),
        _ => writer.null(),
    });
}

/// When the watchdog stops `pod`, and the limit that fires first: idle (none while a call is in
/// flight), maximum runtime, or the balance falling under its floor at the current spend.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn shutdown(
    limits: &WatchdogLimits,
    provider: &ProviderPanel,
    pod: &PodPanel,
    last_call: Option<u64>,
    in_flight: u64,
) -> Option<(u64, &'static str)> {
    let started = pod
        .uptime_s
        .map(|uptime| provider.read_ms.saturating_sub(uptime.saturating_mul(1000)));
    let mut candidates: Vec<(u64, &'static str)> = Vec::new();
    if let Some(idle) = pod.idle_ms.or(limits.idle_ms)
        && in_flight == 0
        && let Some(since) = last_call.max(started)
    {
        candidates.push((since.saturating_add(idle), "idle"));
    }
    if let (Some(max), Some(started)) = (limits.max_runtime_ms, started) {
        candidates.push((started.saturating_add(max), "max-runtime"));
    }
    if let (Some(reserve), Some(balance), Some(spend)) = (
        limits.min_balance_usd,
        provider.balance_usd,
        provider.spend_per_hour_usd,
    ) && spend > 0.0
    {
        let hours = ((balance - reserve) / spend).max(0.0);
        let at = provider.read_ms as f64 + hours * 3_600_000.0;
        if at.is_finite() {
            candidates.push((at as u64, "balance"));
        }
    }
    candidates.into_iter().min_by_key(|(at, _)| *at)
}

fn pod_record(
    writer: &mut Writer,
    pod: &PodPanel,
    ready: Option<bool>,
    last_call: Option<u64>,
    in_flight: u64,
    shutdown: Option<(u64, &'static str)>,
) {
    record(writer, &POD, |writer, key| match key {
        "id" => writer.string(&pod.id),
        "name" => writer.string(&pod.name),
        "model" => optional_string(writer, pod.model.as_deref()),
        "state" => writer.string(&pod.state),
        "gpu" => optional_string(writer, pod.gpu.as_deref()),
        "cost_per_hour_usd" => optional_decimal(writer, pod.cost_per_hour_usd),
        "uptime_s" => optional_number(writer, pod.uptime_s),
        "vllm_ready" => match ready {
            Some(ready) => writer.boolean(ready),
            None => writer.null(),
        },
        "last_call_ms" => optional_number(writer, last_call),
        "in_flight" => writer.number(in_flight),
        "shutdown" => match shutdown {
            Some((at, limit)) => record(writer, &POD_SHUTDOWN, |writer, key| match key {
                "at_ms" => writer.number(at),
                "limit" => writer.string(limit),
                _ => writer.null(),
            }),
            None => writer.null(),
        },
        "external" => writer.boolean(pod.external),
        _ => writer.null(),
    });
}

/// The page's TypeScript types, generated from [`SNAPSHOT_TYPES`]: the text
/// `dashboard/src/generated/snapshot.ts` holds.
pub fn typescript() -> String {
    let mut out = String::from(
        "// Generated by llm-gateway-docs from llm_gateway::dashboard. Do not edit.\n",
    );
    for kind in SNAPSHOT_TYPES {
        let _ = write!(out, "\nexport interface {} {{\n", kind.name);
        for (key, ts) in kind.fields {
            let _ = writeln!(out, "  {key}: {ts};");
        }
        out.push_str("}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        Dashboard, Finished, Observed, PodPanel, ProviderPanel, SNAPSHOT_TYPES, typescript,
    };
    use crate::{
        metrics::{Disposition, Outcome, UsageRecord},
        relay::Wire,
        usage::Usage,
    };
    use std::{net::IpAddr, sync::Arc};

    fn headers<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<&'a str> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| *value)
        }
    }

    fn record(model: &str, duration_ms: u64) -> UsageRecord {
        UsageRecord {
            model: Some(model.to_owned()),
            wire: Wire::Messages,
            disposition: Disposition::Relayed,
            outcome: Outcome::Ok,
            refusal: None,
            status: 200,
            target_status: Some(200),
            reported_model: None,
            input_tokens: None,
            output_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            reasoning_output_tokens: None,
            response_bytes: 10,
            duration_ms,
            profile: None,
        }
    }

    #[test]
    fn a_session_is_a_hash_and_never_the_header_value() {
        let peer: Option<IpAddr> = Some("127.0.0.1".parse().unwrap());
        let claude = Observed::read(headers(&[
            ("x-claude-code-session-id", "6f1d3c1e-secret-session"),
            ("user-agent", "claude-cli/2.1.318 (external, sdk-cli)"),
            ("x-stainless-retry-count", "2"),
        ]))
        .at(peer);
        assert_eq!(claude.label, "claude-code");
        assert_eq!(claude.session.len(), 12);
        assert!(!claude.session.contains("secret"));
        assert_eq!(claude.client_retry, Some(2));
        let codex = Observed::read(headers(&[
            ("session-id", "019a-codex"),
            ("user-agent", "codex_cli_rs/0.160.0"),
        ]))
        .at(peer);
        assert_eq!(codex.label, "codex");
        assert_ne!(codex.session, claude.session);
        let other = Observed::read(headers(&[("user-agent", "curl/8.0")])).at(peer);
        let elsewhere = Observed::read(headers(&[("user-agent", "curl/8.0")]))
            .at(Some("127.0.0.2".parse().unwrap()));
        assert_eq!(other.label, "curl");
        assert_ne!(
            other.session, elsewhere.session,
            "the address is part of the key"
        );
    }

    #[test]
    fn rates_are_input_over_ttft_and_output_over_streaming_time_per_session() {
        let dashboard = Arc::new(Dashboard::default());
        let observed = Observed::read(headers(&[("x-claude-code-session-id", "s")]));
        for (input, output, ttft, duration) in [(1000, 200, 500, 2500), (3000, 600, 1500, 7500)] {
            let progress = dashboard.begin(Wire::Messages, &observed);
            progress.route("qwen");
            dashboard.finish(
                &record("qwen", duration),
                Finished {
                    id: progress.id(),
                    started_ms: 0,
                    upstream: Some("pod-8000.proxy.example".to_owned()),
                    upstream_model: Some("qwen3-coder".to_owned()),
                    queue_wait_ms: Some(5),
                    ttft_ms: Some(ttft),
                    usage: Usage {
                        input_tokens: Some(input),
                        output_tokens: Some(output),
                        cache_read_tokens: Some(0),
                        cache_creation_tokens: None,
                    },
                    body_digest: Some([7; 32]),
                    client_disconnected: false,
                    upstream_retry_after: None,
                    attempts: Vec::new(),
                    credential: None,
                    usage_note: None,
                    observed: observed.clone(),
                },
            );
        }
        let snapshot = dashboard.snapshot();
        // 4000 input tokens over 2 s of TTFT; 800 output tokens over 8 s of streaming.
        assert!(snapshot.contains("\"input_tps\":2000.000"), "{snapshot}");
        assert!(snapshot.contains("\"output_tps\":100.000"), "{snapshot}");
        assert!(snapshot.contains("\"repeated_body\":true"), "{snapshot}");
        assert!(snapshot.contains("\"in_flight\":[]"), "{snapshot}");
        let metrics = dashboard.render_prometheus();
        assert!(
            metrics.contains("llmgw_tokens_total{model=\"qwen\",direction=\"input\"} 4000\n"),
            "{metrics}"
        );
        assert!(
            metrics.contains("direction=\"output\"} 100.000\n"),
            "{metrics}"
        );
        assert!(metrics.contains("llmgw_repeated_bodies_total{model=\"qwen\"} 1\n"));
    }

    /// A cached Claude call: 2 uncached, 4000 cache-read and 300 cache-write input tokens with
    /// a TTFT of 1 s, then one call whose 17 output tokens came in a 5 ms burst and one whose
    /// 400 came over 2 s.
    #[test]
    fn input_rates_count_cached_tokens_and_burst_outputs_are_not_rated() {
        let dashboard = Arc::new(Dashboard::default());
        let observed = Observed::read(headers(&[("x-claude-code-session-id", "c")]));
        for (output, duration) in [(17, 1005), (400, 3000)] {
            let progress = dashboard.begin(Wire::Messages, &observed);
            progress.route("claude");
            dashboard.finish(
                &record("claude", duration),
                Finished {
                    id: progress.id(),
                    started_ms: super::now_ms(),
                    upstream: None,
                    upstream_model: None,
                    queue_wait_ms: None,
                    ttft_ms: Some(1000),
                    usage: Usage {
                        input_tokens: Some(2),
                        output_tokens: Some(output),
                        cache_read_tokens: Some(4000),
                        cache_creation_tokens: Some(300),
                    },
                    body_digest: None,
                    client_disconnected: false,
                    upstream_retry_after: None,
                    attempts: Vec::new(),
                    credential: None,
                    usage_note: None,
                    observed: observed.clone(),
                },
            );
        }
        let snapshot = dashboard.snapshot();
        let calls = &snapshot
            [snapshot.find("\"calls\":").unwrap()..snapshot.find("\"in_flight\":").unwrap()];
        assert_eq!(
            calls.matches("\"input_tps\":4302.000").count(),
            2,
            "(2 + 4000 + 300) tokens over 1 s: {calls}"
        );
        assert!(calls.contains("\"output_tps\":200.000"), "{calls}");
        assert!(
            calls.contains("\"output_tps\":null"),
            "the burst is not rated: {calls}"
        );
        let routes = &snapshot[snapshot.find("\"routes\":").unwrap()..];
        assert!(
            routes.contains("\"input_tps\":4302.000,\"output_tps\":200.000"),
            "the route's average leaves the unrated call out: {routes}"
        );
        let kpi = &snapshot[snapshot.find("\"route_rates\":").unwrap()..];
        assert!(
            kpi.contains("\"route\":\"claude\",\"input_tps\":4302.000,\"output_tps\":200.000"),
            "{kpi}"
        );
        let metrics = dashboard.render_prometheus();
        assert!(
            metrics.contains("llmgw_output_tps_count{route=\"claude\"} 1\n"),
            "{metrics}"
        );
        assert!(
            metrics.contains("llmgw_input_tps_count{route=\"claude\"} 2\n"),
            "{metrics}"
        );
    }

    /// An answer carrying Anthropic's unified limit headers, as a subscription sees them.
    #[test]
    fn subscription_limits_are_kept_per_credential_label_and_served_as_gauges() {
        let dashboard = Arc::new(Dashboard::default());
        let observed = Observed::read(headers(&[]));
        let progress = dashboard.begin(Wire::Messages, &observed);
        let reset = super::now_ms() / 1000 + 3600;
        let limit =
            |name: &str, value: String| (format!("anthropic-ratelimit-unified-{name}"), value);
        dashboard.finish(
            &record("claude", 10),
            Finished {
                id: progress.id(),
                started_ms: 0,
                upstream: None,
                upstream_model: None,
                queue_wait_ms: None,
                ttft_ms: None,
                usage: Usage::default(),
                body_digest: None,
                client_disconnected: false,
                upstream_retry_after: None,
                attempts: vec![super::Attempt {
                    number: 1,
                    status: Some(200),
                    headers: vec![
                        limit("status", "allowed_warning".to_owned()),
                        limit("5h-utilization", "0.42".to_owned()),
                        limit("5h-reset", reset.to_string()),
                        limit("7d-utilization", "0.9".to_owned()),
                        limit("overage-status", "rejected".to_owned()),
                        ("request-id".to_owned(), "req_1".to_owned()),
                    ],
                    ..super::Attempt::default()
                }],
                credential: Some(super::credential_label(b"Bearer sk-ant-oat01-x")),
                usage_note: None,
                observed,
            },
        );
        drop(progress);
        let label = super::credential_label(b"Bearer sk-ant-oat01-x");
        let snapshot = dashboard.snapshot();
        let limits = &snapshot[snapshot.find("\"limits\":").unwrap()..];
        assert!(
            limits.contains(&format!("\"credential\":\"{label}\"")),
            "{limits}"
        );
        assert!(
            limits.contains("\"anthropic-ratelimit-unified-5h-utilization\":\"0.42\""),
            "{limits}"
        );
        assert!(!limits.contains("request-id"), "{limits}");
        assert!(!snapshot.contains("sk-ant"), "{snapshot}");
        let metrics = dashboard.render_prometheus();
        for sample in [
            format!("llmgw_anthropic_utilization{{window=\"5h\",credential=\"{label}\"}} 0.42\n"),
            format!("llmgw_anthropic_utilization{{window=\"7d\",credential=\"{label}\"}} 0.9\n"),
            format!(
                "llmgw_anthropic_status{{window=\"unified\",credential=\"{label}\",status=\"allowed_warning\"}} 1\n"
            ),
            format!(
                "llmgw_anthropic_status{{window=\"overage\",credential=\"{label}\",status=\"rejected\"}} 1\n"
            ),
        ] {
            assert!(metrics.contains(&sample), "{sample} in {metrics}");
        }
        let reset_line =
            format!("llmgw_anthropic_reset_seconds{{window=\"5h\",credential=\"{label}\"}} ");
        let left: u64 = metrics[metrics.find(&reset_line).unwrap() + reset_line.len()..]
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!((3590..=3600).contains(&left), "{left}");
    }

    #[test]
    fn every_declared_field_is_written_in_order_and_nothing_else() {
        let dashboard = Arc::new(Dashboard::default());
        let observed = Observed::read(headers(&[]));
        let progress = dashboard.begin(Wire::Chat, &observed);
        dashboard.finish(
            &record("m", 1),
            Finished {
                id: 1,
                started_ms: 0,
                upstream: None,
                upstream_model: None,
                queue_wait_ms: None,
                ttft_ms: None,
                usage: Usage::default(),
                body_digest: None,
                client_disconnected: true,
                upstream_retry_after: Some("30".to_owned()),
                attempts: vec![super::Attempt {
                    number: 1,
                    status: Some(429),
                    headers: vec![
                        ("retry-after".to_owned(), "30".to_owned()),
                        (
                            "anthropic-ratelimit-unified-status".to_owned(),
                            "allowed".to_owned(),
                        ),
                    ],
                    ..super::Attempt::default()
                }],
                observed,
                credential: Some("0a1b2c3d".to_owned()),
                usage_note: None,
            },
        );
        dashboard.set_provider(Some(ProviderPanel {
            name: "runpod".to_owned(),
            pods: vec![PodPanel {
                id: "p".to_owned(),
                model: Some("m".to_owned()),
                ..PodPanel::default()
            }],
            ..ProviderPanel::default()
        }));
        dashboard.set_watchdog(Some(super::WatchdogLimits {
            idle_ms: Some(600_000),
            ..super::WatchdogLimits::default()
        }));
        dashboard.set_upstream(
            "m",
            Some(super::UpstreamStats {
                source: "vllm".to_owned(),
                ..super::UpstreamStats::default()
            }),
        );
        let snapshot = dashboard.snapshot();
        drop(progress);
        for kind in SNAPSHOT_TYPES {
            let keys: Vec<String> = kind
                .fields
                .iter()
                .map(|(key, _)| format!("\"{key}\":"))
                .collect();
            let first = snapshot
                .find(&keys[0])
                .unwrap_or_else(|| panic!("{} is written: {snapshot}", kind.name));
            let mut at = first;
            for key in &keys {
                let next = snapshot[at..]
                    .find(key.as_str())
                    .unwrap_or_else(|| panic!("{} writes {key} in order: {snapshot}", kind.name));
                at += next;
            }
        }
        let ts = typescript();
        assert!(ts.contains("export interface PodRow {\n  id: string;\n"));
        assert!(ts.starts_with("// Generated by llm-gateway-docs"));
    }
}
