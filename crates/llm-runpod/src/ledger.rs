//! The pool's own spend ledger (story:spend-cap-counts-earlier-pods, `spec/domains/runpod.yaml`
//! `SpendLedger`): what each of this pool's pods is estimated to have cost, kept where a restart
//! finds it again.
//!
//! Runpod's billing reports a pod's spend hours after it ran, so a ceiling checked against the
//! reported spend alone admits starts against money already spent. The ledger holds one entry
//! per pod this pool created: its model's declared price per hour, the instant its create was
//! submitted and the instant its stop was confirmed, if it was. A pod's estimated spend over a
//! stretch of time is its price times the part of its run that falls in that stretch, a pod with
//! no confirmed stop running up to now. The spend check takes, for each ceiling, the larger of
//! the reported spend and the ledger's ([`crate::RunpodProvider::admit_spend`]).
//!
//! The ledger is read once, when the pool is given its store, and written whole after every
//! change. A store that cannot be read, or a ledger that is not one of [`LEDGER_FORMAT`], leaves
//! the ledger unknown: it is never written over, and every metered start is refused `unknown`.
//! A write that fails does the same until a later write succeeds.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use serde_json::{Map, Value, json};

use crate::spend::{DAY_MS, estimate};

/// The format every ledger names.
pub const LEDGER_FORMAT: &str = "llm-gateway.spend-ledger/1";

/// How far back the ledger counts for the total ceiling, and how long it keeps a stopped pod:
/// the 30 days up to now, the window Runpod reports its pod billing over.
pub const TOTAL_WINDOW_MS: u64 = 30 * DAY_MS;

/// The ledger's store could not be read or written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerUnavailable;

impl std::fmt::Display for LedgerUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the spend ledger could not be read or written")
    }
}

impl std::error::Error for LedgerUnavailable {}

/// Where a pool keeps its ledger, as text.
pub trait LedgerStore: Send {
    /// The ledger as last saved; `Ok(None)` when none was ever saved.
    ///
    /// # Errors
    ///
    /// [`LedgerUnavailable`] when a ledger exists and cannot be read.
    fn load(&mut self) -> Result<Option<String>, LedgerUnavailable>;

    /// Replaces the saved ledger with `text`, whole.
    ///
    /// # Errors
    ///
    /// [`LedgerUnavailable`] when it was not saved.
    fn save(&mut self, text: &str) -> Result<(), LedgerUnavailable>;
}

/// One pod of the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    /// The deployment the pod was created for, as the pool named it.
    pub deployment: String,
    /// The request id of the pod's create, which its environment carries: unique across
    /// restarts, where a deployment id is not.
    pub request: String,
    /// The Runpod pod id; `None` for a create whose answer was lost.
    pub pod: Option<String>,
    /// The model alias the pod serves.
    pub model: String,
    /// The model's declared price per hour when the pod was created, micro-USD.
    pub price_per_hour_micro_usd: u64,
    /// When the create was submitted, Unix ms.
    pub started_ms: u64,
    /// When the pod's stop was confirmed, Unix ms; `None` while it may still run.
    pub stopped_ms: Option<u64>,
}

impl LedgerEntry {
    /// What the pod cost between `from_ms` and `now_ms`, rounded up to the next micro-USD.
    fn spent(&self, from_ms: u64, now_ms: u64) -> u64 {
        let end = self.stopped_ms.unwrap_or(now_ms).min(now_ms);
        let start = self.started_ms.max(from_ms);
        estimate(self.price_per_hour_micro_usd, end.saturating_sub(start))
    }

    fn to_json(&self) -> Value {
        json!({
            "deployment": self.deployment,
            "request": self.request,
            "pod": self.pod,
            "model": self.model,
            "price_per_hour_micro_usd": self.price_per_hour_micro_usd,
            "started_ms": self.started_ms,
            "stopped_ms": self.stopped_ms,
        })
    }

    fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.len() != 7 {
            return None;
        }
        let text = |key: &str| object.get(key)?.as_str().map(str::to_owned);
        let optional = |key: &str| match object.get(key)? {
            Value::Null => Some(None),
            other => Some(Some(other)),
        };
        let pod = match optional("pod")? {
            None => None,
            Some(pod) => Some(pod.as_str()?.to_owned()),
        };
        let stopped_ms = match optional("stopped_ms")? {
            None => None,
            Some(at) => Some(at.as_u64()?),
        };
        let entry = Self {
            deployment: text("deployment")?,
            request: text("request")?,
            pod,
            model: text("model")?,
            price_per_hour_micro_usd: object.get("price_per_hour_micro_usd")?.as_u64()?,
            started_ms: object.get("started_ms")?.as_u64()?,
            stopped_ms,
        };
        entry
            .stopped_ms
            .is_none_or(|stopped| stopped >= entry.started_ms)
            .then_some(entry)
    }
}

/// The ledger's text: `{"format": LEDGER_FORMAT, "pods": [<entry>, …]}`.
pub fn encode_ledger(entries: &[LedgerEntry]) -> String {
    let pods: Vec<Value> = entries.iter().map(LedgerEntry::to_json).collect();
    let mut text = json!({"format": LEDGER_FORMAT, "pods": pods}).to_string();
    text.push('\n');
    text
}

/// The entries of a ledger's text; `None` unless it is exactly one ledger of
/// [`LEDGER_FORMAT`]: no other key, every entry whole, no stop before its start.
pub fn decode_ledger(text: &str) -> Option<Vec<LedgerEntry>> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object: &Map<String, Value> = value.as_object()?;
    if object.len() != 2 || object.get("format")?.as_str()? != LEDGER_FORMAT {
        return None;
    }
    object
        .get("pods")?
        .as_array()?
        .iter()
        .map(LedgerEntry::from_json)
        .collect()
}

/// A ledger store in memory, for tests. Clones share one store, so a second pool built on a
/// clone reads what the first one wrote, as a restarted gateway reads its file.
#[derive(Debug, Clone, Default)]
pub struct MemoryLedger {
    state: Arc<Mutex<MemoryState>>,
}

#[derive(Debug, Default)]
struct MemoryState {
    text: Option<String>,
    fail_reads: bool,
    fail_writes: bool,
    writes: u32,
}

impl MemoryLedger {
    /// An empty store: no ledger saved yet.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The text last saved, or set.
    pub fn text(&self) -> Option<String> {
        self.state().text.clone()
    }

    /// Replaces the stored text, as an operator editing the file would.
    pub fn set_text(&self, text: Option<String>) {
        self.state().text = text;
    }

    /// The stored ledger's entries; `None` when there is none or it is not a ledger.
    pub fn entries(&self) -> Option<Vec<LedgerEntry>> {
        decode_ledger(&self.text()?)
    }

    /// Reads from now on fail.
    pub fn fail_reads(&self, fail: bool) {
        self.state().fail_reads = fail;
    }

    /// Writes from now on fail and store nothing.
    pub fn fail_writes(&self, fail: bool) {
        self.state().fail_writes = fail;
    }

    /// Saves that were stored.
    pub fn writes(&self) -> u32 {
        self.state().writes
    }
}

impl LedgerStore for MemoryLedger {
    fn load(&mut self) -> Result<Option<String>, LedgerUnavailable> {
        let state = self.state();
        if state.fail_reads {
            return Err(LedgerUnavailable);
        }
        Ok(state.text.clone())
    }

    fn save(&mut self, text: &str) -> Result<(), LedgerUnavailable> {
        let mut state = self.state();
        if state.fail_writes {
            return Err(LedgerUnavailable);
        }
        state.text = Some(text.to_owned());
        state.writes = state.writes.saturating_add(1);
        Ok(())
    }
}

/// The ledger a provider keeps: its entries, and whether they are known to be what its store
/// holds.
pub(crate) struct Ledger {
    store: Box<dyn LedgerStore>,
    entries: Vec<LedgerEntry>,
    /// The store was read, and held a ledger or none; `false` forbids every write.
    loaded: bool,
    /// The last write succeeded, or none was needed yet.
    written: bool,
}

impl Ledger {
    /// The ledger `store` holds.
    pub(crate) fn open(mut store: Box<dyn LedgerStore>) -> Self {
        let entries = match store.load() {
            Ok(None) => Some(Vec::new()),
            Ok(Some(text)) => decode_ledger(&text),
            Err(LedgerUnavailable) => None,
        };
        Self {
            store,
            loaded: entries.is_some(),
            entries: entries.unwrap_or_default(),
            written: true,
        }
    }

    /// Whether the entries are what the store holds: read, and every write since stored.
    pub(crate) const fn sound(&self) -> bool {
        self.loaded && self.written
    }

    /// What every pod not in `live` (request ids) cost between `from_ms` and `now_ms`.
    pub(crate) fn spent(&self, from_ms: u64, now_ms: u64, live: &BTreeSet<String>) -> u64 {
        self.entries
            .iter()
            .filter(|entry| !live.contains(&entry.request))
            .fold(0, |sum: u64, entry| {
                sum.saturating_add(entry.spent(from_ms, now_ms))
            })
    }

    /// Records a submitted create.
    pub(crate) fn open_pod(&mut self, entry: LedgerEntry, now_ms: u64) {
        if !self.loaded {
            return;
        }
        self.entries.push(entry);
        self.save(now_ms);
    }

    /// Records the confirmed stop of the pod `pod_id`, if the ledger holds it running.
    pub(crate) fn close_pod(&mut self, pod_id: &str, now_ms: u64) {
        self.close_where(now_ms, |entry| entry.pod.as_deref() == Some(pod_id));
    }

    /// Records as stopped now every running entry that `running` says no longer runs.
    pub(crate) fn close_where(&mut self, now_ms: u64, stopped: impl Fn(&LedgerEntry) -> bool) {
        if !self.loaded {
            return;
        }
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.stopped_ms.is_none() && stopped(entry) {
                entry.stopped_ms = Some(now_ms.max(entry.started_ms));
                changed = true;
            }
        }
        if changed {
            self.save(now_ms);
        }
    }

    /// Drops every pod stopped before the total window and writes the ledger whole.
    fn save(&mut self, now_ms: u64) {
        let horizon = now_ms.saturating_sub(TOTAL_WINDOW_MS);
        self.entries
            .retain(|entry| entry.stopped_ms.is_none_or(|stopped| stopped >= horizon));
        self.written = self.store.save(&encode_ledger(&self.entries)).is_ok();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{Ledger, LedgerEntry, MemoryLedger, decode_ledger, encode_ledger};

    fn entry(request: &str, started_ms: u64, stopped_ms: Option<u64>) -> LedgerEntry {
        LedgerEntry {
            deployment: "qwen-1".to_owned(),
            request: request.to_owned(),
            pod: Some(format!("pod-{request}")),
            model: "qwen".to_owned(),
            price_per_hour_micro_usd: 3_600_000,
            started_ms,
            stopped_ms,
        }
    }

    #[test]
    fn a_ledger_reads_back_what_was_written_and_nothing_else() {
        let entries = vec![
            entry("a", 1_000, Some(2_000)),
            LedgerEntry {
                pod: None,
                ..entry("b", 5_000, None)
            },
        ];
        let text = encode_ledger(&entries);
        assert_eq!(decode_ledger(&text), Some(entries));
        assert_eq!(
            decode_ledger(r#"{"format":"llm-gateway.spend-ledger/1","pods":[]}"#),
            Some(Vec::new())
        );
        for refused in [
            "",
            "not json",
            r#"{"format":"other/1","pods":[]}"#,
            r#"{"format":"llm-gateway.spend-ledger/1","pods":[],"more":1}"#,
            r#"{"format":"llm-gateway.spend-ledger/1","pods":[{"deployment":"d"}]}"#,
            r#"{"format":"llm-gateway.spend-ledger/1","pods":[{"deployment":"d","request":"r","pod":null,"model":"m","price_per_hour_micro_usd":1,"started_ms":5,"stopped_ms":4}]}"#,
            r#"{"format":"llm-gateway.spend-ledger/1","pods":[{"deployment":"d","request":"r","pod":null,"model":"m","price_per_hour_micro_usd":-1,"started_ms":5,"stopped_ms":null}]}"#,
        ] {
            assert_eq!(decode_ledger(refused), None, "{refused}");
        }
    }

    #[test]
    fn a_pod_counts_the_part_of_its_run_inside_the_stretch_asked() {
        let store = MemoryLedger::new();
        let mut ledger = Ledger::open(Box::new(store.clone()));
        // 1 USD per second of run, in micro-USD per hour.
        ledger.open_pod(entry("a", 10_000, None), 10_000);
        ledger.open_pod(entry("b", 0, Some(4_000)), 10_000);
        let none = BTreeSet::new();
        assert_eq!(ledger.spent(0, 12_000, &none), 2_000 + 4_000);
        assert_eq!(ledger.spent(11_000, 12_000, &none), 1_000, "from 11 s on");
        assert_eq!(
            ledger.spent(0, 12_000, &BTreeSet::from(["a".to_owned()])),
            4_000
        );
        ledger.close_pod("pod-a", 13_000);
        assert_eq!(
            ledger.spent(0, 20_000, &none),
            3_000 + 4_000,
            "stopped at 13 s"
        );
        assert_eq!(store.entries().unwrap()[0].stopped_ms, Some(13_000));
    }

    #[test]
    fn a_ledger_that_cannot_be_read_is_never_written_over() {
        let store = MemoryLedger::new();
        store.set_text(Some("not a ledger".to_owned()));
        let mut ledger = Ledger::open(Box::new(store.clone()));
        assert!(!ledger.sound());
        ledger.open_pod(entry("a", 0, None), 0);
        assert_eq!(store.text().as_deref(), Some("not a ledger"));
        assert_eq!(store.writes(), 0);
    }

    #[test]
    fn a_pod_stopped_before_the_window_is_dropped_on_the_next_write() {
        let store = MemoryLedger::new();
        let mut ledger = Ledger::open(Box::new(store.clone()));
        ledger.open_pod(entry("old", 0, Some(1_000)), 1_000);
        let later = super::TOTAL_WINDOW_MS + 1_001;
        ledger.open_pod(entry("new", later, None), later);
        let kept: Vec<_> = store
            .entries()
            .unwrap()
            .into_iter()
            .map(|entry| entry.request)
            .collect();
        assert_eq!(kept, ["new"]);
    }
}
