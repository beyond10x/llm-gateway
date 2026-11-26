//! The id of every model call and the owner's record of it (story:call-id-and-usage-record;
//! `CallHeader` and `CallRoute` in `spec/domains/gateway.yaml`, `CallRecord` in
//! `spec/domains/telemetry.yaml`, docs/gateway.md "Call ids and call records").
//!
//! An id is given out when a request that will make a usage record has had its head decided.
//! Its record is `in-flight` from then and `ended` with the usage record's values once the last
//! byte or the refusal is written. The store holds every in-flight call and the last
//! [`MAX_ENDED_CALLS`] ended ones; its lock is held only to change or copy an entry, never while
//! an answer is written or a target read.

use crate::{
    error::RefusalCode,
    json,
    metrics::{Disposition, UsageRecord},
    relay::Wire,
};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    hash::{BuildHasher, Hasher, RandomState},
    sync::{
        Arc, Mutex, OnceLock, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// The header every answer to a model call carries its id in.
pub(crate) const HEADER: &str = "x-llm-gateway-call";
/// The path prefix of a call's record: `/v1/calls/{id}`.
pub(crate) const PREFIX: &str = "/v1/calls/";
/// The `format` a call's record names.
const FORMAT: &str = "llm-gateway.call/1";
/// How many ended calls the gateway holds, the oldest dropped first (docs/gateway.md "Bounds",
/// `ended-calls-held`).
pub(crate) const MAX_ENDED_CALLS: usize = 1024;

/// The part of every id this process gives out: 32 lowercase hexadecimal digits from two
/// hashes keyed by the standard library's per-process random keys, which it seeds from the
/// operating system's random source, with the start time hashed in.
fn process_part() -> &'static str {
    static PART: OnceLock<String> = OnceLock::new();
    PART.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let drawn = |salt: u8| {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u128(nanos);
            hasher.write_u8(salt);
            hasher.finish()
        };
        format!("{:016x}{:016x}", drawn(0), drawn(1))
    })
}

/// How many ids this process has given out, plus one: the counter part of the next id. It
/// starts at 1, so an id ending `-0` is of the form and never given out.
static NEXT: AtomicU64 = AtomicU64::new(1);

/// A new id, never given out before by this process.
fn new_id() -> String {
    format!(
        "{}-{:x}",
        process_part(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// One held call.
enum Entry {
    InFlight {
        wire: Wire,
        model: Option<String>,
        principal: Option<String>,
    },
    /// Boxed: a record is several times the size of an in-flight entry.
    Ended(Box<UsageRecord>),
}

#[derive(Default)]
struct Held {
    entries: HashMap<String, Entry>,
    /// The ended calls' ids, the oldest first.
    ended: VecDeque<String>,
}

/// The calls one gateway holds.
#[derive(Default)]
pub(crate) struct Calls {
    held: Mutex<Held>,
}

impl fmt::Debug for Calls {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Calls").finish_non_exhaustive()
    }
}

impl Calls {
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives out a new id for a call on `wire` and holds it `in-flight`.
    pub(crate) fn begin(calls: &Arc<Self>, wire: Wire, principal: Option<String>) -> Ticket {
        let id = new_id();
        calls.held().entries.insert(
            id.clone(),
            Entry::InFlight {
                wire,
                model: None,
                principal,
            },
        );
        Ticket {
            calls: Arc::clone(calls),
            id,
            ended: false,
        }
    }

    /// The record of call `id` as `GET /v1/calls/{id}` answers it, or `None` when the gateway
    /// does not hold it.
    pub(crate) fn render(&self, id: &str) -> Option<String> {
        let held = self.held();
        let entry = held.entries.get(id)?;
        let mut writer = json::Writer::new();
        writer.begin_object();
        writer.key("format");
        writer.string(FORMAT);
        writer.key("id");
        writer.string(id);
        match entry {
            Entry::InFlight {
                wire,
                model,
                principal,
            } => {
                writer.key("state");
                writer.string("in-flight");
                optional_string(&mut writer, "model", model.as_deref());
                writer.key("wire");
                writer.string(wire.label());
                optional_string(&mut writer, "principal", principal.as_deref());
            }
            Entry::Ended(record) => ended(&mut writer, record),
        }
        writer.end_object();
        Some(writer.finish())
    }
}

/// An ended call's keys after `id`, in the published order; a value the record does not carry
/// is left out.
fn ended(writer: &mut json::Writer, record: &UsageRecord) {
    writer.key("state");
    writer.string("ended");
    optional_string(writer, "model", record.model.as_deref());
    writer.key("wire");
    writer.string(record.wire.label());
    writer.key("disposition");
    writer.string(match record.disposition {
        Disposition::Relayed => "relayed",
        Disposition::Refused => "refused",
        Disposition::UpstreamFailed => "upstream-failed",
    });
    writer.key("outcome");
    writer.string(record.outcome.label());
    optional_string(writer, "refusal", record.refusal.map(RefusalCode::wire));
    writer.key("status");
    writer.number(u64::from(record.status));
    optional_number(writer, "target_status", record.target_status.map(u64::from));
    optional_number(writer, "input_tokens", record.input_tokens);
    optional_number(writer, "output_tokens", record.output_tokens);
    optional_number(writer, "cache_read_tokens", record.cached_input_tokens);
    optional_number(
        writer,
        "cache_creation_tokens",
        record.cache_creation_input_tokens,
    );
    optional_string(writer, "cost_usd", record.cost_usd.as_deref());
    writer.key("response_bytes");
    writer.number(record.response_bytes);
    writer.key("duration_ms");
    writer.number(record.duration_ms);
    optional_string(writer, "profile", record.profile.as_deref());
    optional_string(writer, "principal", record.principal.as_deref());
}

fn optional_string(writer: &mut json::Writer, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        writer.key(key);
        writer.string(value);
    }
}

fn optional_number(writer: &mut json::Writer, key: &str, value: Option<u64>) {
    if let Some(value) = value {
        writer.key(key);
        writer.number(value);
    }
}

/// One call's id and its place in the store. Dropped without [`Ticket::end`], the call is
/// forgotten: it made no usage record.
pub(crate) struct Ticket {
    calls: Arc<Calls>,
    id: String,
    ended: bool,
}

impl fmt::Debug for Ticket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Ticket").field(&self.id).finish()
    }
}

impl Ticket {
    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// The body named the relayed model `alias`.
    pub(crate) fn model(&self, alias: &str) {
        if let Some(Entry::InFlight { model, .. }) = self.calls.held().entries.get_mut(&self.id) {
            *model = Some(alias.to_owned());
        }
    }

    /// The call ended with `record`; the oldest ended call past [`MAX_ENDED_CALLS`] is dropped.
    pub(crate) fn end(mut self, record: &UsageRecord) {
        self.ended = true;
        let mut held = self.calls.held();
        held.entries
            .insert(self.id.clone(), Entry::Ended(Box::new(record.clone())));
        held.ended.push_back(self.id.clone());
        while held.ended.len() > MAX_ENDED_CALLS {
            if let Some(oldest) = held.ended.pop_front() {
                held.entries.remove(&oldest);
            }
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.ended {
            self.calls.held().entries.remove(&self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_of_the_published_form_and_never_repeat() {
        let ids: Vec<String> = (0..64).map(|_| new_id()).collect();
        for id in &ids {
            assert!((1..=64).contains(&id.len()), "{id}");
            assert!(
                id.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                "{id}"
            );
        }
        let distinct: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(distinct.len(), ids.len());
    }
}
