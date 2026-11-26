//! Adversary pass 2 on story:interaction-record-store (wave 2026-11-26-w05, unit U3), on its
//! correction 1: the schema check before the writer opens the store (D94) and the failures the
//! writer counts at the stop (D96 with `llmgw_interaction_records_failed_total`).
//!
//! Each test runs the shipped binary on port 0 against a fake messages endpoint on loopback and
//! reads the store with `rusqlite`.

use crate::support::{Fixture, OWNER_SECRET};
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
const MESSAGE: &str = r#"{"id":"msg_adv","type":"message","role":"assistant","model":"claude","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":1}}"#;
const FAILED_WARNING: &str = "the writer failed to store a call";
const NOT_STORED: &str = "b10x-llm-gateway: interaction records: ";

/// A loopback messages endpoint answering every request with [`MESSAGE`].
fn upstream() -> SocketAddr {
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
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{MESSAGE}",
                MESSAGE.len()
            );
            drop(connection.write_all(answer.as_bytes()));
        }
    });
    address
}

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

/// The status of the answer to a messages call of `session`.
fn post(gateway: &Gateway, session: &str) -> u16 {
    let body = r#"{"model":"claude","max_tokens":16,"messages":[{"role":"user","content":"hi"}]}"#;
    let request = format!(
        "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nx-claude-code-session-id: {session}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(gateway.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    String::from_utf8_lossy(&raw)
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
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

/// A store an earlier writer left: `meta` with `schema` = `version`, and one wire record.
const EARLIER: &str = "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
     CREATE TABLE wire_steps (session_id TEXT NOT NULL, seq INTEGER NOT NULL,
       harness TEXT NOT NULL, step TEXT NOT NULL, PRIMARY KEY (session_id, seq));
     INSERT INTO wire_steps VALUES ('0123456789ab', 0, 'ClaudeCode', 'earlier');";

fn wire_records_kept(store: &Path) -> i64 {
    rusqlite::Connection::open(store)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM wire_steps WHERE session_id = '0123456789ab'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

/// D94 guards sessionspec's `Store::open`, which empties the store unless the `schema` row is
/// the string `SCHEMA_VERSION.to_string()` ("4"). The check parses the row as an integer
/// instead, so a row of "04" (or "+4") passes the check as version 4 and is then emptied by
/// the open the check exists to guard.
#[test]
fn a_schema_row_that_parses_as_four_but_is_not_the_string_four_is_never_emptied() {
    let fixture = Fixture::new("adv2-records-schema-04");
    let store = store_in(&fixture);
    {
        let earlier = rusqlite::Connection::open(&store).unwrap();
        earlier.execute_batch(EARLIER).unwrap();
        earlier
            .execute("INSERT INTO meta VALUES ('schema', '04')", [])
            .unwrap();
    }
    let config = document(&fixture, upstream(), &store);
    let started = start(&config, &fixture.dir);
    let refused = started.as_ref().err().cloned();
    drop(started);
    assert_eq!(
        wire_records_kept(&store),
        1,
        "the start (refused: {refused:?}) emptied a store whose schema row reads \"04\""
    );
}

/// D94 maps every error of its read of the `schema` row to "is no session store". A store of
/// this gateway's own schema version that another connection holds locked past the check's
/// 2 s busy timeout is refused under that reason, which tells the operator the file is foreign.
/// The start may be refused; it must not name a session store of the right version "no session
/// store".
#[test]
fn a_session_store_held_locked_at_start_is_never_refused_as_no_session_store() {
    let fixture = Fixture::new("adv2-records-locked-at-start");
    let store = store_in(&fixture);
    {
        let earlier = rusqlite::Connection::open(&store).unwrap();
        earlier.execute_batch(EARLIER).unwrap();
        earlier
            .execute(
                "INSERT INTO meta VALUES ('schema', ?1)",
                [sessionspec_records::store::SCHEMA_VERSION.to_string()],
            )
            .unwrap();
    }
    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    let config = document(&fixture, upstream(), &store);
    let started = start(&config, &fixture.dir);
    let stderr = started.as_ref().err().cloned().unwrap_or_default();
    drop(started);
    lock.execute_batch("COMMIT;").unwrap();
    drop(lock);
    assert!(
        !stderr.contains("no session store"),
        "a store of schema version {} held locked by another connection was refused as no \
         session store: {stderr}",
        sessionspec_records::store::SCHEMA_VERSION
    );
    assert_eq!(wire_records_kept(&store), 1);
}

/// The guide: a call the writer failed to store is counted and logged with one warning per
/// call; D96: a call not stored by the end of the stop's wait is named on the stop's line. A
/// call that fails while the stop waits is neither: the thread that reads the writer's failure
/// count reads it every 100 ms and ends without a last read once the gateway's shutdown has
/// dropped the adapter, nothing reads the count after the wait, and the stop's line names only
/// calls still queued (`queued - written - failed`). Here the writer is held
/// by a lock while three calls are answered, the store is removed, and the lock is released
/// during the wait, so the calls fail then.
#[test]
fn a_call_that_fails_to_store_during_the_stop_is_warned_or_named_at_the_stop() {
    let fixture = Fixture::new("adv2-records-fail-at-stop");
    let store = store_in(&fixture);
    let config = document(&fixture, upstream(), &store);
    let mut gateway = start(&config, &fixture.dir).unwrap();
    thread::sleep(Duration::from_millis(800));
    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.busy_timeout(Duration::from_secs(5)).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    for session in ["stop-a", "stop-b", "stop-c"] {
        assert_eq!(post(&gateway, session), 200);
    }
    for suffix in ["", "-wal", "-shm"] {
        drop(std::fs::remove_file(format!("{}{suffix}", store.display())));
    }
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&gateway.child),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    thread::sleep(Duration::from_millis(400));
    drop(lock.execute_batch("COMMIT;"));
    drop(lock);
    let status = gateway.child.wait().unwrap();
    let mut lines = Vec::new();
    while let Ok(line) = gateway.lines.recv_timeout(Duration::from_secs(2)) {
        lines.push(line);
    }
    let stored_in_new_file: i64 = rusqlite::Connection::open(&store)
        .ok()
        .and_then(|connection| {
            connection
                .query_row("SELECT count(*) FROM wire_steps", [], |row| row.get(0))
                .ok()
        })
        .unwrap_or(0);
    let warned = lines
        .iter()
        .filter(|line| line.contains(FAILED_WARNING))
        .count();
    let named: usize = lines
        .iter()
        .find_map(|line| {
            line.strip_prefix(NOT_STORED)?
                .strip_suffix(" calls not stored at stop")?
                .parse()
                .ok()
        })
        .unwrap_or(0);
    assert!(status.success(), "{status}: {lines:?}");
    assert_eq!(stored_in_new_file, 0, "{lines:?}");
    assert!(
        warned + named >= 2,
        "at least the two calls queued behind the held one failed during the stop (the store \
         was removed); warned {warned}, named {named}: {lines:?}"
    );
}
