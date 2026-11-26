//! Adversary pass on story:interaction-record-store (wave 2026-11-26-w05, unit U3): what the
//! binary does with a session store it did not make, a write the writer thread could not make,
//! a refused call whose body is not JSON, and the help text of the skipped series.
//!
//! Each test runs the shipped binary on port 0 against a fake messages endpoint on loopback and
//! reads the store with `rusqlite` or with `agenticspec-records`' own reader.

use crate::support::{Fixture, OWNER_SECRET};
use agenticspec_records::{claude::Cmd, store::Store, wire::session_hash};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const STORED: Duration = Duration::from_secs(4);

/// A loopback messages endpoint answering every request with `head_extra` and `body`.
fn upstream(head_extra: &'static str, body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            if !read_request(&mut connection) {
                continue;
            }
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n{head_extra}content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            drop(connection.write_all(answer.as_bytes()));
        }
    });
    address
}

const MESSAGE: &str = r#"{"id":"msg_adv","type":"message","role":"assistant","model":"claude","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#;

fn read_request(connection: &mut TcpStream) -> bool {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    let end = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return false,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    let length: usize = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while raw.len() < end + 4 + length {
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    }
    true
}

struct Gateway {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
}

/// Starts the binary on `config`; `Err` with its stderr when it exits before listening.
fn start(config: &Path, directory: &Path) -> Result<Gateway, String> {
    let home = directory.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .current_dir(directory)
        .env_remove("RUST_LOG")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + STARTUP;
    let mut written = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(line) = lines.recv_timeout(left) else {
            drop(child.kill());
            drop(child.wait());
            return Err(written.join("\n"));
        };
        written.push(line.clone());
        if let Some(address) = line.strip_prefix(LISTENING) {
            return Ok(Gateway {
                child,
                address: address.parse().unwrap(),
                lines,
            });
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

struct Answer {
    status: u16,
    head: String,
}

impl Answer {
    fn call_id(&self) -> Option<String> {
        self.head.split("\r\n").find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("x-llm-gateway-call")
                .then(|| value.trim().to_owned())
        })
    }
}

fn exchange(address: SocketAddr, request: &str) -> (Answer, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    let end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no answer head: {:?}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap();
    let body = String::from_utf8_lossy(&raw[end + 4..]).into_owned();
    (Answer { status, head }, body)
}

fn post(gateway: &Gateway, session: &str, body: &str) -> Answer {
    exchange(
        gateway.address,
        &format!(
            "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nx-claude-code-session-id: {session}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .0
}

fn metrics(gateway: &Gateway) -> String {
    exchange(
        gateway.address,
        &format!(
            "GET /metrics HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nconnection: close\r\n\r\n"
        ),
    )
    .1
}

/// The sum of every `llmgw_interaction_records_*` counter on `/metrics`.
fn records_counters(text: &str) -> u64 {
    text.lines()
        .filter(|line| line.starts_with("llmgw_interaction_records_"))
        .filter_map(|line| line.rsplit(' ').next()?.trim().parse::<u64>().ok())
        .sum()
}

fn counter(text: &str, name: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{name} ")))
        .unwrap_or_else(|| panic!("/metrics has no {name}: {text}"))
        .trim()
        .parse()
        .unwrap()
}

fn first_turn(text: &str) -> String {
    format!(
        r#"{{"model":"claude","max_tokens":16,"messages":[{{"role":"user","content":"{text}"}}]}}"#
    )
}

fn document(fixture: &Fixture, upstream: SocketAddr, store: &Path) -> PathBuf {
    let secret = fixture.owner_secret();
    fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[hosted.claude]\nwires = [\"messages\"]\nbase_url = \"http://{upstream}\"\nauth_kind = \"anonymous\"\n\n[interaction_records]\npath = \"{}\"\n",
        secret.display(),
        store.display()
    ))
}

fn store_in(fixture: &Fixture) -> PathBuf {
    let directory = fixture.path("records");
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("sessions.db")
}

/// The call ids of the model turns stored for `hash`, once `done` holds; panics after [`STORED`].
fn stored_calls(store: &Path, hash: &str, done: impl Fn(&[String]) -> bool) -> Vec<String> {
    let deadline = Instant::now() + STORED;
    loop {
        let calls: Vec<String> = Store::open(store)
            .unwrap()
            .wire_steps(hash)
            .unwrap()
            .iter()
            .filter_map(|step| match &step.cmd {
                Cmd::RecordModelTurn(turn) => turn.call_id.clone(),
                _ => None,
            })
            .collect();
        if done(&calls) {
            return calls;
        }
        assert!(
            Instant::now() < deadline,
            "the store never held what was expected: {calls:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

/// The guide recommends naming the gateway's file in sessionspec's state directory, the store
/// `sessionspec sync` writes, and promises that a file that does not open as a session store
/// refuses the start. A store another sessionspec version wrote (schema version 3 here) is
/// neither refused nor kept: `Store::open` drops its tables, the gateway's own `wire_steps`
/// included, which nothing can fill again.
#[test]
fn a_store_of_another_schema_version_is_refused_or_kept_never_emptied_at_start() {
    let fixture = Fixture::new("adv-records-other-version");
    let store = store_in(&fixture);
    {
        let earlier = rusqlite::Connection::open(&store).unwrap();
        earlier
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta VALUES ('schema', '3');
                 CREATE TABLE wire_steps (session_id TEXT NOT NULL, seq INTEGER NOT NULL,
                   harness TEXT NOT NULL, step TEXT NOT NULL, PRIMARY KEY (session_id, seq));
                 INSERT INTO wire_steps VALUES ('0123456789ab', 0, 'ClaudeCode', 'earlier');",
            )
            .unwrap();
    }
    let address = upstream("", MESSAGE);
    let config = document(&fixture, address, &store);
    let started = start(&config, &fixture.dir);
    let refused = started.is_err();
    drop(started);
    let kept: i64 = rusqlite::Connection::open(&store)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM wire_steps WHERE session_id = '0123456789ab'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert_eq!(
        kept, 1,
        "the binary's start (refused: {refused}) emptied the wire records a store of schema \
         version 3 held"
    );
}

/// A write the writer thread fails (the file removed or rotated while the gateway runs, a full
/// disk, a lock held past SQLite's 10 s busy timeout by `sessionspec sync` on the shared file)
/// loses the call. The guide lists what is left out and counts each; this loss is counted by no
/// series on `/metrics` (sessionspec's `Stats::failed` is never read).
#[test]
fn a_call_the_writer_failed_to_store_is_counted_on_metrics() {
    let fixture = Fixture::new("adv-records-write-failed");
    let store = store_in(&fixture);
    let address = upstream("", MESSAGE);
    let config = document(&fixture, address, &store);
    let gateway = start(&config, &fixture.dir).unwrap();
    let hash = session_hash("claude-code", "write-failed");

    let first = post(&gateway, "write-failed", &first_turn("first"));
    assert_eq!(first.status, 200);
    let first_call = first.call_id().unwrap();
    stored_calls(&store, &hash, |calls| calls.contains(&first_call));
    let before = records_counters(&metrics(&gateway));

    for suffix in ["", "-wal", "-shm"] {
        drop(std::fs::remove_file(format!("{}{suffix}", store.display())));
    }
    let second = post(&gateway, "write-failed", &first_turn("second"));
    assert_eq!(second.status, 200);

    let deadline = Instant::now() + STORED;
    loop {
        let text = metrics(&gateway);
        if records_counters(&text) > before {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the second call was not stored (the store file was removed) and no \
             llmgw_interaction_records_* series counted it: {}",
            text.lines()
                .filter(|line| line.starts_with("llmgw_interaction_records_"))
                .collect::<Vec<_>>()
                .join("; ")
        );
        thread::sleep(Duration::from_millis(50));
    }
}

/// The story's design: a refused call is lifted as an incomplete turn with the reason
/// `refused`. A call refused `body-not-json` has a call id and is neither stored nor counted:
/// its lift returns no step, and `AppenderHandle::append` reports an empty batch as queued.
#[test]
fn a_call_refused_for_a_body_that_is_not_json_is_stored_refused_or_counted_skipped() {
    let fixture = Fixture::new("adv-records-not-json");
    let store = store_in(&fixture);
    let address = upstream("", MESSAGE);
    let config = document(&fixture, address, &store);
    let gateway = start(&config, &fixture.dir).unwrap();
    let hash = session_hash("claude-code", "not-json");
    let before = metrics(&gateway);

    let refused = post(
        &gateway,
        "not-json",
        r#"{"model":"claude","messages":[ not json"#,
    );
    assert_eq!(refused.status, 400, "{}", refused.head);
    let refused_call = refused
        .call_id()
        .expect("a call refused after its head carries a call id");

    // A good call after it, in the same session: once it is stored, the writer has taken
    // everything queued before it.
    let good = post(&gateway, "not-json", &first_turn("after"));
    assert_eq!(good.status, 200);
    let good_call = good.call_id().unwrap();
    let calls = stored_calls(&store, &hash, |calls| calls.contains(&good_call));

    let after = metrics(&gateway);
    let skipped = counter(&after, "llmgw_interaction_records_skipped_total")
        - counter(&before, "llmgw_interaction_records_skipped_total");
    assert!(
        calls.contains(&refused_call) || skipped == 1,
        "the refused call {refused_call} is not in the store ({calls:?}) and skipped rose by \
         {skipped}"
    );
}

/// The binary counts a call whose answer did not decode (`CallBytes::Undecodable`, records.rs:
/// every case but `Copied`) as skipped, and `docs/gateway.md` says so in the series' row; the
/// help text `/metrics` serves for the series, and the output pasted in
/// `website/docs/guides/scrape-the-counters.md`, still say only "over the bound".
#[test]
fn the_skipped_series_help_names_an_answer_that_did_not_decode_as_the_contract_does() {
    let contract = include_str!("../../../docs/gateway.md");
    let row = contract
        .lines()
        .find(|line| line.starts_with("| `llmgw_interaction_records_skipped_total` |"))
        .unwrap();
    assert!(row.contains("not decoded"), "{row}");

    let fixture = Fixture::new("adv-records-skipped-help");
    let store = store_in(&fixture);
    let address = upstream("", MESSAGE);
    let config = document(&fixture, address, &store);
    let gateway = start(&config, &fixture.dir).unwrap();
    let text = metrics(&gateway);
    let help = text
        .lines()
        .find(|line| line.starts_with("# HELP llmgw_interaction_records_skipped_total "))
        .unwrap();
    assert!(
        help.contains("decod"),
        "docs/gateway.md: {row}\n/metrics: {help}"
    );
}

/// The writer's join handle is dropped (records.rs: "nothing joins it"), so the calls still in
/// its queue at SIGTERM die with the process: the binary exits 0, prints its usual stop line,
/// and neither the guide nor any line or counter says that answered calls were not stored. The
/// store is held locked while the calls run, as the unit's own queue test does, so that the
/// queue is not empty when the signal arrives; without the lock a probe of 40 calls lost 22.
#[test]
fn every_call_answered_before_sigterm_is_stored_or_the_stop_names_the_calls_it_lost() {
    const SENT: usize = 8;
    let fixture = Fixture::new("adv-records-sigterm");
    let store = store_in(&fixture);
    let address = upstream("", MESSAGE);
    let config = document(&fixture, address, &store);
    let mut gateway = start(&config, &fixture.dir).unwrap();

    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.busy_timeout(Duration::from_secs(5)).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    for n in 0..SENT {
        let answer = post(&gateway, &format!("term-{n}"), &first_turn("x"));
        assert_eq!(answer.status, 200);
    }
    let dropped = counter(
        &metrics(&gateway),
        "llmgw_interaction_records_dropped_total",
    );
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&gateway.child),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let status = gateway.child.wait().unwrap();
    let mut stderr = Vec::new();
    while let Ok(line) = gateway.lines.recv_timeout(Duration::from_secs(2)) {
        stderr.push(line);
    }
    lock.execute_batch("COMMIT;").unwrap();
    drop(lock);

    let stored: i64 = rusqlite::Connection::open(&store)
        .unwrap()
        .query_row(
            "SELECT count(DISTINCT session_id) FROM wire_steps",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dropped, 0, "every call was queued");
    let named = stderr.iter().any(|line| line.contains("interaction"));
    assert!(
        stored == i64::try_from(SENT).unwrap() || named,
        "{SENT} calls answered and queued, {stored} stored after SIGTERM ({status}); stderr: \
         {stderr:?}"
    );
}
