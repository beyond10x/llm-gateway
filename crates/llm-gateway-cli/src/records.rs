//! The interaction records (story:interaction-record-store; `spec/domains/deployment.yaml`
//! `InteractionRecordsDeclaration`): every finished call lifted into sessionspec's session
//! records (`sessionspec_records::wire::lift_call`) and appended to a SQLite file by one writer
//! thread (`sessionspec_records::append::Appender`).
//!
//! The adapter implements the gateway's [`FinishedCalls`] port. It is called on the connection's
//! thread and never waits: the lift is a pure function of the call and the session's state, and
//! the append only queues. A full queue drops the call and counts it; a call whose bytes were not
//! copied (over the bound, or an answer that was not decoded), or whose lift yields no step, is
//! not queued and is counted skipped; a call the writer fails to store is counted failed. Each
//! session's lift state is held for at most `sessions_held` sessions, the least recently used
//! forgotten first: a forgotten session's next call continues under the same agent id, without
//! the chain state of its earlier turns.
//!
//! The port hands the bytes by reference (`Option<&CallBytes>`), and the lift takes them owned
//! (`WireCall`), so they are copied once more here; moving them needs the port to hand them by
//! value, a change of the gateway crate.

use crate::{
    config::InteractionRecordsConfig,
    refusal::{Refusal, StartupRefusal},
};
use llm_gateway::{
    AnswerBytes, CallBytes, CallEnding, FinishedCall, FinishedCalls, Harness, Metrics, Relay, Wire,
};
use rusqlite::{OpenFlags, OptionalExtension};
use sessionspec_records::{
    append::{Appender, AppenderHandle, Stats},
    store::SCHEMA_VERSION,
    wire::{self, Answer, GatewayFacts, WireCall, WireSession},
};
use sessionspec_types::session as s;
use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, Mutex, PoisonError, Weak, atomic::Ordering},
    thread,
    time::{Duration, Instant},
};

/// How long the stop waits for the writer to store its queue (D96).
const DRAIN: Duration = Duration::from_secs(2);
/// How often the writer's failure count is read.
const FAILURES_READ: Duration = Duration::from_millis(100);

/// The relay with the adapter and the byte copy when the document declares
/// `[interaction_records]`, and the relay unchanged without it: then no byte is copied, no
/// writer thread runs and no file is created. With it, also what the stop waits on.
///
/// # Errors
/// `config:value` naming `interaction_records.path`, never the path itself, when its directory
/// does not exist, the file is no session store or holds another schema version, or it does
/// not open.
pub(crate) fn compose(
    relay: Relay,
    config: Option<&InteractionRecordsConfig>,
    metrics: &Arc<Metrics>,
) -> Result<(Relay, Option<Drain>), Refusal> {
    Ok(match start(config, metrics)? {
        Some((records, bound)) => {
            let drain = Drain {
                appender: records.appender.clone(),
            };
            (
                relay.with_finished_calls(records).with_call_bytes(bound),
                Some(drain),
            )
        }
        None => (relay, None),
    })
}

/// The adapter and its byte bound; `None` without the table.
fn start(
    config: Option<&InteractionRecordsConfig>,
    metrics: &Arc<Metrics>,
) -> Result<Option<(Arc<InteractionRecords>, usize)>, Refusal> {
    let Some(config) = config else {
        return Ok(None);
    };
    let directory = match config.path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if !directory.is_dir() {
        return Err(path_refusal("names a directory that does not exist"));
    }
    check_store(&config.path)?;
    let (appender, writer) = Appender::spawn(&config.path, config.queue)
        .map_err(|_| path_refusal("cannot be opened as a session store"))?;
    // The writer ends once every handle is dropped and its queue is drained; nothing joins it.
    // The stop waits for its queue through `Drain` instead.
    drop(writer);
    let records = Arc::new(InteractionRecords {
        appender,
        metrics: Arc::clone(metrics),
        held: Mutex::new(Held {
            capacity: config.sessions_held,
            tick: 0,
            sessions: HashMap::new(),
            by_use: BTreeMap::new(),
        }),
    });
    watch_failures(Arc::downgrade(&records))
        .map_err(|_| path_refusal("cannot start the thread that counts its failures"))?;
    Ok(Some((records, config.call_bytes)))
}

fn path_refusal(rule: &str) -> Refusal {
    Refusal::new(
        StartupRefusal::ConfigValue,
        format!("interaction_records.path {rule}"),
    )
}

/// D94: refuses a file at `path` that sessionspec's open would empty: one that is no session
/// store, or holds another schema version. A missing or empty file is a new store. Only reads.
fn check_store(path: &Path) -> Result<(), Refusal> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(path_refusal("cannot be read")),
        Ok(file) if file.len() == 0 => return Ok(()),
        Ok(_) => {}
    }
    let no_store = || path_refusal("is no session store");
    let connection = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| path_refusal("cannot be read"))?;
    connection
        .busy_timeout(DRAIN)
        .map_err(|_| path_refusal("cannot be read"))?;
    let version: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|_| no_store())?;
    let version: i64 = version
        .and_then(|version| version.parse().ok())
        .ok_or_else(no_store)?;
    if version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(path_refusal(&format!(
            "holds schema version {version}; this gateway writes schema version {SCHEMA_VERSION}"
        )))
    }
}

/// Counts each call the writer failed to store in `llmgw_interaction_records_failed_total`,
/// read from the writer's own failure count, with one warning line per call naming the series
/// and never the path. The thread ends with the adapter.
fn watch_failures(records: Weak<InteractionRecords>) -> std::io::Result<()> {
    thread::Builder::new()
        .name("records-failures".to_owned())
        .spawn(move || {
            let mut counted = 0;
            loop {
                thread::sleep(FAILURES_READ);
                let Some(records) = records.upgrade() else {
                    return;
                };
                let failed = records.appender.stats().failed.load(Ordering::Relaxed);
                for _ in counted..failed {
                    records.metrics.count_interaction_record_failed();
                    tracing::warn!(
                        "interaction records: the writer failed to store a call, counted in \
                         llmgw_interaction_records_failed_total"
                    );
                }
                counted = counted.max(failed);
            }
        })
        .map(drop)
}

/// What the stop waits on: the writer's counts (D96).
pub(crate) struct Drain {
    appender: AppenderHandle,
}

impl Drain {
    /// Waits at most 2 s for the writer to store every queued call; the calls still queued then.
    pub(crate) fn finish(&self) -> u64 {
        let deadline = Instant::now() + DRAIN;
        loop {
            let left = queued(self.appender.stats());
            if left == 0 || Instant::now() >= deadline {
                return left;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// Calls queued and neither stored nor failed yet.
fn queued(stats: &Stats) -> u64 {
    let done = stats
        .written
        .load(Ordering::Relaxed)
        .saturating_add(stats.failed.load(Ordering::Relaxed));
    stats.queued.load(Ordering::Relaxed).saturating_sub(done)
}

/// The [`FinishedCalls`] adapter.
pub(crate) struct InteractionRecords {
    appender: AppenderHandle,
    metrics: Arc<Metrics>,
    held: Mutex<Held>,
}

/// One session's lift state, shared by its calls.
type Session = Arc<Mutex<WireSession>>;

/// The sessions whose lift state is held, at most `capacity`, by last use.
struct Held {
    capacity: usize,
    tick: u64,
    /// By session hash: the session's last use and its state.
    sessions: HashMap<String, (u64, Session)>,
    /// Session hash by last use, the least recent first.
    by_use: BTreeMap<u64, String>,
}

impl Held {
    /// The state of the session `hash`, marked used now; a session not held is started anew,
    /// and the least recently used one is forgotten when `capacity` are held.
    fn session(&mut self, hash: &str) -> Session {
        self.tick += 1;
        let tick = self.tick;
        if let Some((used, session)) = self.sessions.get_mut(hash) {
            self.by_use.remove(used);
            *used = tick;
            self.by_use.insert(tick, hash.to_owned());
            return Arc::clone(session);
        }
        if self.sessions.len() >= self.capacity
            && let Some((_, oldest)) = self.by_use.pop_first()
        {
            self.sessions.remove(&oldest);
        }
        let session = Arc::new(Mutex::new(WireSession::new()));
        self.sessions
            .insert(hash.to_owned(), (tick, Arc::clone(&session)));
        self.by_use.insert(tick, hash.to_owned());
        session
    }
}

impl FinishedCalls for InteractionRecords {
    fn finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>) {
        // Only a copy can be lifted: a call over the bound, one whose answer was not decoded,
        // and any other case without bytes is left out and counted. The copy is cloned: the
        // port lends it, the lift takes it owned.
        let Some(CallBytes::Copied { request, answer }) = bytes else {
            self.metrics.count_interaction_record_skipped();
            return;
        };
        let lifted = WireCall {
            wire: match call.wire {
                Wire::Messages => wire::Wire::Messages,
                Wire::Chat => wire::Wire::Chat,
                Wire::Responses => wire::Wire::Responses,
            },
            request: request.clone(),
            answer: match answer {
                Some(AnswerBytes::Events(events)) => Answer::Events(events.clone()),
                Some(AnswerBytes::Body(body)) => Answer::Body(body.clone()),
                None => Answer::Body(Vec::new()),
            },
            facts: facts(call),
        };
        let session = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .session(&call.session_hash);
        // Lifted and queued under the session's lock, so its steps reach the writer in the
        // order they were lifted.
        let mut state = session.lock().unwrap_or_else(PoisonError::into_inner);
        let steps = wire::lift_call(&mut state, &lifted);
        // D95: a request the lift cannot read yields no step; `append` would take the empty
        // batch as queued, so it is counted here and never handed on.
        if steps.is_empty() {
            self.metrics.count_interaction_record_skipped();
            return;
        }
        if !self
            .appender
            .append(&call.session_hash, lifted.facts.harness, steps)
        {
            self.metrics.count_interaction_record_dropped();
        }
    }
}

/// sessionspec's `GatewayFacts` of a finished call (`spec/domains/telemetry.yaml`, the
/// `BOUNDARY:` comment of `FinishedCall`).
fn facts(call: &FinishedCall) -> GatewayFacts {
    GatewayFacts {
        call_id: call.call_id.clone(),
        session_hash: call.session_hash.clone(),
        harness: match call.harness {
            Harness::ClaudeCode => s::Harness::ClaudeCode,
            Harness::Codex => s::Harness::Codex,
            Harness::Other => s::Harness::Other,
        },
        path: call.path.clone(),
        route: call.route.clone(),
        upstream: call.upstream.clone(),
        attempts: i64::from(call.attempts),
        queue_wait_ms: call.queue_wait_ms.map(millis),
        ttft_ms: call.ttft_ms.map(millis),
        declared_cost_microdollars: call.declared_cost_usd.as_deref().and_then(microdollars),
        incomplete: match call.ending {
            CallEnding::Complete => None,
            CallEnding::ClientDisconnected => Some(s::IncompleteReason::ClientDisconnected),
            CallEnding::UpstreamCut => Some(s::IncompleteReason::UpstreamCut),
            CallEnding::WireError => Some(s::IncompleteReason::WireError),
            CallEnding::Refused(_) => Some(s::IncompleteReason::Refused),
        },
        started_ms: millis(call.started_ms),
        finished_ms: millis(call.finished_ms),
    }
}

fn millis(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The usage record's decimal cost in USD as whole microdollars, read by llm's own amount
/// parser and rounded up; `None` for a value it refuses.
fn microdollars(usd: &str) -> Option<i64> {
    let amount = llm_cost::Amount::parse(usd).ok()?;
    i64::try_from(amount.nanos().div_ceil(1000)).ok()
}

#[cfg(test)]
mod tests {
    use super::{Held, microdollars, start};
    use llm_gateway::Metrics;
    use std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
    };

    /// Acceptance 3 at the composition: without the table there is no adapter and no bound, so
    /// the relay is composed without `with_finished_calls` and `with_call_bytes`.
    #[test]
    fn without_the_table_there_is_no_adapter_and_no_byte_bound() {
        let started = start(None, &Arc::new(Metrics::default())).unwrap();
        assert!(started.is_none());
    }

    #[test]
    fn the_least_recently_used_session_is_forgotten_first() {
        let mut held = Held {
            capacity: 2,
            tick: 0,
            sessions: HashMap::new(),
            by_use: BTreeMap::new(),
        };
        let a = held.session("a");
        let b = held.session("b");
        assert!(Arc::ptr_eq(&a, &held.session("a")));
        drop(held.session("c"));
        assert!(Arc::ptr_eq(&a, &held.session("a")), "a was used last");
        assert!(!Arc::ptr_eq(&b, &held.session("b")), "b was forgotten");
        assert_eq!(held.sessions.len(), 2);
        assert_eq!(held.by_use.len(), 2);
    }

    #[test]
    fn a_cost_is_whole_microdollars_rounded_up() {
        assert_eq!(microdollars("0.000123"), Some(123));
        assert_eq!(microdollars("0.0000001"), Some(1));
        assert_eq!(microdollars("1.5"), Some(1_500_000));
        assert_eq!(microdollars("-1"), None);
    }
}
