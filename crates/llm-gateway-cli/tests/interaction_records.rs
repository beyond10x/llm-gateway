//! story:interaction-record-store: with `[interaction_records]` the binary lifts every finished
//! call into sessionspec's session records and appends them to a SQLite file; without it the
//! relay copies nothing, no writer thread runs and no file is created.
//!
//! Every test runs the shipped binary on port 0 against a fake messages endpoint on loopback and
//! reads the store back with `sessionspec-records`' own reader (`Store::wire_steps`).

use crate::support::{Fixture, OWNER_SECRET};
use sessionspec_records::{
    claude::{Cmd, Step},
    store::{Store, command_text},
    wire::session_hash,
};
use std::{
    collections::VecDeque,
    fmt::Write as _,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
/// How long a test waits for the writer thread to store what it was handed.
const STORED: Duration = Duration::from_secs(4);
/// The name the writer thread carries, as `/proc` shows it (15 bytes).
const WRITER_THREAD: &str = "sessionspec-app";

// --- The fake messages endpoint ------------------------------------------------------------------

/// An answer the endpoint writes whole, then closes the connection.
#[derive(Clone)]
struct Reply {
    content_type: &'static str,
    body: String,
}

fn json(body: &str) -> Reply {
    Reply {
        content_type: "application/json",
        body: body.to_owned(),
    }
}

/// A server-sent event stream: one `event:` and `data:` pair per `(name, data)`.
fn events(pieces: &[(&str, &str)]) -> Reply {
    let mut body = String::new();
    for (name, data) in pieces {
        let _ = write!(body, "event: {name}\ndata: {data}\n\n");
    }
    Reply {
        content_type: "text/event-stream",
        body,
    }
}

/// A finished messages answer with `id` and the text `text`.
fn message(id: &str, text: &str) -> Reply {
    json(&format!(
        r#"{{"id":"{id}","type":"message","role":"assistant","model":"claude","content":[{{"type":"text","text":"{text}"}}],"stop_reason":"end_turn","usage":{{"input_tokens":3,"output_tokens":1}}}}"#
    ))
}

/// A loopback messages endpoint: a token count is answered `{"input_tokens":3}`, every other
/// request with the next scripted reply, or a short message once the script is spent.
struct Upstream {
    address: SocketAddr,
    paths: Arc<Mutex<Vec<String>>>,
}

impl Upstream {
    fn start(script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let paths = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&paths);
        let mut script: VecDeque<Reply> = script.into();
        thread::spawn(move || {
            let mut served = 0_usize;
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                connection
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let Some(path) = read_request_path(&mut connection) else {
                    continue;
                };
                seen.lock().unwrap().push(path.clone());
                let reply = if path.ends_with("/count_tokens") {
                    json(r#"{"input_tokens":3}"#)
                } else {
                    served += 1;
                    script
                        .pop_front()
                        .unwrap_or_else(|| message(&format!("msg_default_{served}"), "ok"))
                };
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    reply.content_type,
                    reply.body.len(),
                    reply.body
                );
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self { address, paths }
    }

    fn paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

/// Reads one request whole and returns its path.
fn read_request_path(connection: &mut TcpStream) -> Option<String> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 8192];
    let end = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let length: usize = head
        .to_ascii_lowercase()
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while raw.len() < end + 4 + length {
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
    }
    head.split(' ').nth(1).map(str::to_owned)
}

// --- The gateway process -------------------------------------------------------------------------

struct Gateway {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
}

impl Gateway {
    /// Starts the shipped binary on `config` in `directory`, with `HOME` and the XDG directories
    /// under it, and waits for its `listening on` line.
    fn start(config: &Path, directory: &Path) -> Self {
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
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                drop(child.wait());
                panic!("never reported {LISTENING:?}; stderr: {written:?}");
            };
            written.push(line.clone());
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
        };
        Self {
            child,
            address,
            lines,
        }
    }

    /// The names of the process's threads, as `/proc` shows them.
    fn threads(&self) -> Vec<String> {
        let tasks = PathBuf::from(format!("/proc/{}/task", self.child.id()));
        std::fs::read_dir(tasks)
            .unwrap()
            .filter_map(|task| {
                std::fs::read_to_string(task.ok()?.path().join("comm"))
                    .ok()
                    .map(|name| name.trim_end().to_owned())
            })
            .collect()
    }

    /// `GET /metrics` as text.
    fn scrape(&self) -> String {
        let answer = exchange(
            self.address,
            &format!(
                "GET /metrics HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nconnection: close\r\n\r\n"
            ),
        );
        String::from_utf8_lossy(&answer.body).into_owned()
    }

    /// One counter of `GET /metrics`.
    fn counter(&self, name: &str) -> u64 {
        let text = self.scrape();
        text.lines()
            .find_map(|line| line.strip_prefix(&format!("{name} ")))
            .unwrap_or_else(|| panic!("/metrics has no {name}: {text}"))
            .trim()
            .parse()
            .unwrap()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
        while self.lines.try_recv().is_ok() {}
    }
}

/// What the gateway answered one request.
struct Answer {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Answer {
    /// The `x-llm-gateway-call` header, when the answer carries one.
    fn call_id(&self) -> Option<String> {
        self.head.split("\r\n").find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("x-llm-gateway-call")
                .then(|| value.trim().to_owned())
        })
    }
}

fn exchange(address: SocketAddr, request: &str) -> Answer {
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
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let rest = &raw[end + 4..];
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    Answer { status, head, body }
}

fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let Some(line_end) = rest.windows(2).position(|window| window == b"\r\n") else {
            return body;
        };
        let size_text = String::from_utf8_lossy(&rest[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap().trim(), 16).unwrap();
        if size == 0 {
            return body;
        }
        let start = line_end + 2;
        body.extend_from_slice(&rest[start..start + size]);
        rest = &rest[start + size + 2..];
    }
}

/// A model call on `path` with `headers` (each line ending `\r\n`) and `body`.
fn post(gateway: &Gateway, path: &str, headers: &str, body: &str) -> Answer {
    exchange(
        gateway.address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{headers}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
}

fn claude_session(id: &str) -> String {
    format!("x-claude-code-session-id: {id}\r\n")
}

/// A messages request on the route `claude` whose conversation is `messages` (a JSON array).
fn request(messages: &str) -> String {
    format!(
        r#"{{"model":"claude","max_tokens":64,"tools":[{{"name":"Task","description":"spawn","input_schema":{{"type":"object"}}}}],"messages":{messages}}}"#
    )
}

fn first_turn(text: &str) -> String {
    request(&format!(r#"[{{"role":"user","content":"{text}"}}]"#))
}

// --- Documents -------------------------------------------------------------------------------

/// A document serving `claude` from `upstream` on the messages wire, with `extra` appended.
fn document(fixture: &Fixture, upstream: &Upstream, extra: &str) -> PathBuf {
    let secret = fixture.owner_secret();
    fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[hosted.claude]\nwires = [\"messages\"]\nbase_url = \"http://{}\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n{extra}",
        secret.display(),
        upstream.address
    ))
}

/// The `[interaction_records]` table storing into `store`, with `lines` after `path`.
fn records_table(store: &Path, lines: &str) -> String {
    format!(
        "\n[interaction_records]\npath = \"{}\"\n{lines}",
        store.display()
    )
}

fn store_in(fixture: &Fixture) -> PathBuf {
    let directory = fixture.path("records");
    std::fs::create_dir_all(&directory).unwrap();
    directory.join("sessions.db")
}

// --- Reading the store ---------------------------------------------------------------------------

fn stored(store: &Path, hash: &str) -> Vec<Step> {
    Store::open(store)
        .unwrap_or_else(|error| panic!("the store does not open: {error}"))
        .wire_steps(hash)
        .unwrap_or_else(|error| panic!("the stored steps do not decode: {error}"))
}

/// The session's stored steps once `done` holds of them; panics after [`STORED`].
fn stored_when(store: &Path, hash: &str, done: impl Fn(&[Step]) -> bool) -> Vec<Step> {
    let deadline = Instant::now() + STORED;
    loop {
        let steps = stored(store, hash);
        if done(&steps) {
            return steps;
        }
        assert!(
            Instant::now() < deadline,
            "the store never held what was expected for {hash}: {} steps {:?}",
            steps.len(),
            names(&steps)
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn names(steps: &[Step]) -> Vec<&'static str> {
    steps
        .iter()
        .map(|step| sessionspec_records::replay::command_name(&step.cmd))
        .collect()
}

fn turns(steps: &[Step]) -> Vec<&sessionspec_types::session::RecordModelTurn> {
    steps
        .iter()
        .filter_map(|step| match &step.cmd {
            Cmd::RecordModelTurn(turn) => Some(turn),
            _ => None,
        })
        .collect()
}

fn agents(steps: &[Step]) -> Vec<&sessionspec_types::session::StartAgent> {
    steps
        .iter()
        .filter_map(|step| match &step.cmd {
            Cmd::StartAgent(start) => Some(start),
            _ => None,
        })
        .collect()
}

// --- Acceptance 2: the table -----------------------------------------------------------------

const MINIMAL: &str = "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[hosted.claude]\nwires = [\"messages\"]\nbase_url = \"http://127.0.0.1:9\"\nauth_kind = \"anonymous\"\n";

#[test]
fn the_table_takes_its_defaults_and_refuses_an_unknown_key_and_each_value_out_of_range() {
    let deployment = llm_gateway_cli::parse(&format!(
        "{MINIMAL}\n[interaction_records]\npath = \"s.db\"\n"
    ))
    .expect("a table naming only its path is accepted");
    let shown = format!("{deployment:?}");
    for default in ["queue: 1024", "call_bytes: 8388608", "sessions_held: 4096"] {
        assert!(
            shown.contains(default),
            "{default} is not the default: {shown}"
        );
    }
    let without = llm_gateway_cli::parse(MINIMAL).unwrap();
    assert!(
        format!("{without:?}").contains("interaction_records: None"),
        "{without:?}"
    );

    for (lines, code, named) in [
        ("path = \"s.db\"\ncolour = 1\n", "config:schema", "colour"),
        ("queue = 8\n", "config:schema", "path"),
        ("path = \"\"\n", "config:value", "interaction_records.path"),
        (
            "path = \"s.db\"\nqueue = 0\n",
            "config:value",
            "interaction_records.queue",
        ),
        (
            "path = \"s.db\"\nqueue = 65537\n",
            "config:value",
            "interaction_records.queue",
        ),
        (
            "path = \"s.db\"\ncall_bytes = 0\n",
            "config:value",
            "interaction_records.call_bytes",
        ),
        (
            "path = \"s.db\"\ncall_bytes = 33554433\n",
            "config:value",
            "interaction_records.call_bytes",
        ),
        (
            "path = \"s.db\"\nsessions_held = 0\n",
            "config:value",
            "interaction_records.sessions_held",
        ),
        (
            "path = \"s.db\"\nsessions_held = 65537\n",
            "config:value",
            "interaction_records.sessions_held",
        ),
    ] {
        let refusal = llm_gateway_cli::parse(&format!("{MINIMAL}\n[interaction_records]\n{lines}"))
            .expect_err(lines);
        assert_eq!(refusal.code(), code, "{lines}: {refusal}");
        assert!(refusal.message().contains(named), "{lines}: {refusal}");
    }
    for lines in [
        "path = \"s.db\"\nqueue = 1\ncall_bytes = 1\nsessions_held = 1\n",
        "path = \"s.db\"\nqueue = 65536\ncall_bytes = 33554432\nsessions_held = 65536\n",
    ] {
        llm_gateway_cli::parse(&format!("{MINIMAL}\n[interaction_records]\n{lines}"))
            .unwrap_or_else(|refusal| panic!("{lines}: {refusal}"));
    }
}

#[test]
fn a_path_whose_directory_is_missing_is_refused_at_start_by_name_and_never_quoted() {
    let fixture = Fixture::new("records-missing-directory");
    let upstream = Upstream::start(Vec::new());
    let store = fixture.path("no-such-directory").join("sessions.db");
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let output = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("refused config:value") && stderr.contains("interaction_records.path"),
        "{stderr}"
    );
    assert!(!stderr.contains("no-such-directory"), "{stderr}");
    assert!(!store.exists() && !store.parent().unwrap().exists());
}

/// The binary's start on `config`: whether it was refused, and its standard error. A binary
/// still running after [`STARTUP`] was not refused; it is stopped.
fn refused_start(config: &Path) -> (bool, String) {
    let log = config.with_extension("stderr");
    let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + STARTUP;
    let refused = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break !status.success();
        }
        if Instant::now() >= deadline || std::fs::read_to_string(&log).unwrap().contains(LISTENING)
        {
            drop(child.kill());
            drop(child.wait());
            break false;
        }
        thread::sleep(Duration::from_millis(10));
    };
    (refused, std::fs::read_to_string(&log).unwrap())
}

/// D94: a SQLite file that is no session store (another program's, here a table `notes` with
/// one row) refuses the start by name and is left as it was; sessionspec's own open would have
/// emptied it.
#[test]
fn a_file_that_is_no_session_store_is_refused_at_start_and_kept() {
    let fixture = Fixture::new("records-no-store");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    {
        let connection = rusqlite::Connection::open(&store).unwrap();
        connection
            .execute_batch("CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('kept');")
            .unwrap();
    }
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let (refused, stderr) = refused_start(&config);
    assert!(refused, "{stderr}");
    assert!(
        stderr.contains("refused config:value")
            && stderr.contains("interaction_records.path")
            && stderr.contains("no session store"),
        "{stderr}"
    );
    assert!(!stderr.contains(&*store.to_string_lossy()), "{stderr}");
    let kept: i64 = rusqlite::Connection::open(&store)
        .unwrap()
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, 1);
}

/// A file sessionspec's open began and did not finish (its `meta` table without a schema row,
/// no other table) is a new store: sessionspec's open completes it and loses nothing.
#[test]
fn a_file_with_only_sessionspecs_meta_table_and_no_schema_row_is_a_new_store() {
    let fixture = Fixture::new("records-meta-only");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    rusqlite::Connection::open(&store)
        .unwrap()
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    let answer = post(
        &gateway,
        "/v1/messages",
        &claude_session("meta-only"),
        &first_turn("hello"),
    );
    assert_eq!(answer.status, 200);
    stored_when(&store, &session_hash("claude-code", "meta-only"), |steps| {
        turns(steps).len() == 1
    });
}

/// A store of this gateway's version that another process holds locked past the check's wait
/// refuses the start with its own reason, never "no session store", and is kept.
#[test]
fn a_store_held_locked_at_start_is_refused_as_held_locked() {
    let fixture = Fixture::new("records-locked-at-start");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    {
        let connection = rusqlite::Connection::open(&store).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL); \
                 CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('kept');",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO meta VALUES ('schema', ?1)",
                [sessionspec_records::store::SCHEMA_VERSION.to_string()],
            )
            .unwrap();
    }
    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let (refused, stderr) = refused_start(&config);
    lock.execute_batch("COMMIT;").unwrap();
    drop(lock);
    assert!(refused, "{stderr}");
    assert!(
        stderr.contains(
            "refused config:value: interaction_records.path is held locked by another process"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains(&*store.to_string_lossy()), "{stderr}");
    let kept: i64 = rusqlite::Connection::open(&store)
        .unwrap()
        .query_row("SELECT count(*) FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(kept, 1);
}

/// D98: with the store removed, 50 calls answered within a second fail to store; the failed
/// series counts each of them, and at most two warning lines name the series.
#[test]
fn a_removed_store_under_load_warns_at_most_once_a_second_and_counts_every_call() {
    const CALLS: u64 = 50;
    let fixture = Fixture::new("records-failures-rate");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    for suffix in ["", "-wal", "-shm"] {
        drop(std::fs::remove_file(format!("{}{suffix}", store.display())));
    }
    let address = gateway.address;
    let body = first_turn("hello");
    let started = Instant::now();
    let calls: Vec<_> = (0..CALLS)
        .map(|n| {
            let raw = format!(
                "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                claude_session(&format!("gone-{n}")),
                body.len()
            );
            thread::spawn(move || exchange(address, &raw).status)
        })
        .collect();
    for call in calls {
        assert_eq!(call.join().unwrap(), 200);
    }
    let answered = started.elapsed();
    assert!(answered < Duration::from_secs(1), "{answered:?}");
    let deadline = Instant::now() + STORED;
    while gateway.counter("llmgw_interaction_records_failed_total") < CALLS {
        assert!(
            Instant::now() < deadline,
            "the series never reached {CALLS}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    // Long enough for a warning the one-second rate held back to be written.
    thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        gateway.counter("llmgw_interaction_records_failed_total"),
        CALLS
    );
    let warnings: Vec<String> = gateway
        .lines
        .try_iter()
        .filter(|line| line.contains("llmgw_interaction_records_failed_total"))
        .collect();
    assert!(
        (1..=2).contains(&warnings.len()),
        "{} warning lines: {warnings:?}",
        warnings.len()
    );
    assert!(
        warnings
            .iter()
            .all(|line| !line.contains(&*store.to_string_lossy())),
        "{warnings:?}"
    );
}

/// D94: a store of another schema version refuses the start naming both versions, never the
/// path; an empty file is a new store, as a missing one is.
#[test]
fn a_store_of_another_schema_version_is_refused_naming_both_versions_and_an_empty_file_is_used() {
    let fixture = Fixture::new("records-schema-version");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    {
        let connection = rusqlite::Connection::open(&store).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); \
                 INSERT INTO meta VALUES ('schema', '3');",
            )
            .unwrap();
    }
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let (refused, stderr) = refused_start(&config);
    assert!(refused, "{stderr}");
    let ours = sessionspec_records::store::SCHEMA_VERSION;
    assert!(
        stderr.contains("refused config:value")
            && stderr.contains("interaction_records.path")
            && stderr.contains("schema version 3")
            && stderr.contains(&format!("schema version {ours}")),
        "{stderr}"
    );
    assert!(!stderr.contains(&*store.to_string_lossy()), "{stderr}");

    std::fs::write(&store, b"").unwrap();
    let gateway = Gateway::start(&config, &fixture.dir);
    let answer = post(
        &gateway,
        "/v1/messages",
        &claude_session("empty-file"),
        &first_turn("hello"),
    );
    assert_eq!(answer.status, 200);
    stored_when(
        &store,
        &session_hash("claude-code", "empty-file"),
        |steps| turns(steps).len() == 1,
    );
}

/// The three interaction-record series are served, the skipped one's help names every cause
/// (over the bound, not decoded, a lift that yields no step), and the output pasted in the
/// counters guide is what `/metrics` serves for each.
#[test]
fn the_three_series_are_served_and_pasted_in_the_counters_guide_as_served() {
    let fixture = Fixture::new("records-series");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    let served = gateway.scrape();
    let guide = include_str!("../../../website/docs/guides/scrape-the-counters.md");
    for series in [
        "llmgw_interaction_records_dropped_total",
        "llmgw_interaction_records_skipped_total",
        "llmgw_interaction_records_failed_total",
    ] {
        let help = served
            .lines()
            .find(|line| line.starts_with(&format!("# HELP {series} ")))
            .unwrap_or_else(|| panic!("/metrics serves no {series}: {served}"));
        assert!(guide.contains(help), "the guide does not paste {help:?}");
    }
    let skipped = served
        .lines()
        .find(|line| line.starts_with("# HELP llmgw_interaction_records_skipped_total "))
        .unwrap();
    for cause in ["over the bound", "not decoded", "no step"] {
        assert!(skipped.contains(cause), "{skipped}");
    }
}

// --- Acceptance 3: without the table ----------------------------------------------------------

fn files_under(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn without_the_table_no_writer_thread_runs_and_no_file_is_created() {
    let fixture = Fixture::new("records-without-table");
    let upstream = Upstream::start(Vec::new());
    let config = document(&fixture, &upstream, "");
    let written = files_under(&fixture.dir);
    let gateway = Gateway::start(&config, &fixture.dir);
    let answer = post(
        &gateway,
        "/v1/messages",
        &claude_session("off"),
        &first_turn("hello"),
    );
    assert_eq!(answer.status, 200);
    assert!(
        !gateway
            .threads()
            .iter()
            .any(|name| name.starts_with(WRITER_THREAD)),
        "{:?}",
        gateway.threads()
    );
    drop(gateway);
    let home = fixture.dir.join("home");
    let after: Vec<PathBuf> = files_under(&fixture.dir)
        .into_iter()
        .filter(|path| !written.contains(path))
        .collect();
    assert!(after.is_empty(), "files created: {after:?}");
    assert!(files_under(&home).is_empty());

    // The same probe, after the same call, sees the writer and the file when the table is
    // declared. A thread takes its name only once it runs, so the probe waits for it.
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    let answer = post(
        &gateway,
        "/v1/messages",
        &claude_session("on"),
        &first_turn("hello"),
    );
    assert_eq!(answer.status, 200);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !gateway
        .threads()
        .iter()
        .any(|name| name.starts_with(WRITER_THREAD))
    {
        assert!(Instant::now() < deadline, "{:?}", gateway.threads());
        thread::sleep(Duration::from_millis(10));
    }
    assert!(store.exists());
}

// --- Acceptance 4: the queue and the byte bound --------------------------------------------------

#[test]
fn with_the_writer_held_every_answer_completes_and_the_calls_beyond_the_queue_are_dropped() {
    const QUEUE: usize = 4;
    let fixture = Fixture::new("records-held-writer");
    let replies: Vec<Reply> = (0..2 * QUEUE)
        .map(|n| message(&format!("msg_held_{n}"), &format!("answer {n}")))
        .collect();
    let upstream = Upstream::start(replies.clone());
    let store = store_in(&fixture);
    let config = document(
        &fixture,
        &upstream,
        &records_table(&store, &format!("queue = {QUEUE}\n")),
    );
    let gateway = Gateway::start(&config, &fixture.dir);
    assert_eq!(
        gateway.counter("llmgw_interaction_records_dropped_total"),
        0
    );

    // An exclusive lock on the store: the writer's next pass waits on it (its busy timeout is
    // 10 s), so it takes nothing from the queue while the calls run.
    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.busy_timeout(Duration::from_secs(5)).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    thread::sleep(Duration::from_millis(800));

    let session = claude_session("held");
    for (n, reply) in replies.iter().enumerate() {
        let answer = post(
            &gateway,
            "/v1/messages",
            &session,
            &first_turn(&format!("call {n}")),
        );
        assert_eq!(answer.status, 200, "call {n}");
        assert_eq!(
            String::from_utf8_lossy(&answer.body),
            reply.body,
            "call {n}'s answer"
        );
    }
    assert_eq!(
        gateway.counter("llmgw_interaction_records_dropped_total"),
        u64::try_from(QUEUE).unwrap()
    );

    // Released, the writer stores what the queue held.
    lock.execute_batch("COMMIT;").unwrap();
    drop(lock);
    let steps = stored_when(&store, &session_hash("claude-code", "held"), |steps| {
        turns(steps).len() == QUEUE
    });
    assert_eq!(turns(&steps).len(), QUEUE);
}

/// D96: with the writer held, the stop waits 2 s for it, then names the calls still queued on
/// one line just before the stop line, which is unchanged, and exits 0.
#[test]
fn a_stop_with_the_writer_held_names_the_calls_not_stored_just_before_the_stop_line() {
    const CALLS: usize = 2;
    let fixture = Fixture::new("records-stop");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let mut gateway = Gateway::start(&config, &fixture.dir);
    let lock = rusqlite::Connection::open(&store).unwrap();
    lock.busy_timeout(Duration::from_secs(5)).unwrap();
    lock.execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN EXCLUSIVE;")
        .unwrap();
    thread::sleep(Duration::from_millis(800));
    for n in 0..CALLS {
        let answer = post(
            &gateway,
            "/v1/messages",
            &claude_session(&format!("stop-{n}")),
            &first_turn("hello"),
        );
        assert_eq!(answer.status, 200);
    }
    assert_eq!(
        gateway.counter("llmgw_interaction_records_dropped_total"),
        0
    );
    let signalled = Instant::now();
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&gateway.child),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let status = gateway.child.wait().unwrap();
    let stopped_after = signalled.elapsed();
    let mut lines = Vec::new();
    while let Ok(line) = gateway.lines.recv_timeout(Duration::from_secs(2)) {
        lines.push(line);
    }
    drop(lock);
    assert!(status.success(), "{status}: {lines:?}");
    let [.., not_stored, stop] = lines.as_slice() else {
        panic!("{lines:?}");
    };
    assert_eq!(
        not_stored,
        &format!("b10x-llm-gateway: interaction records: {CALLS} calls not stored at stop")
    );
    assert!(
        stop.starts_with("b10x-llm-gateway: stopped by SIGTERM accepted="),
        "{lines:?}"
    );
    assert!(
        stopped_after >= Duration::from_secs(2) && stopped_after < Duration::from_secs(5),
        "{stopped_after:?}"
    );
}

#[test]
fn a_call_over_call_bytes_is_answered_unchanged_stored_never_and_counted_skipped() {
    let fixture = Fixture::new("records-over-bound");
    let big = message("msg_big", &"x".repeat(4096));
    let small = message("msg_small", "ok");
    let upstream = Upstream::start(vec![big.clone(), small]);
    let store = store_in(&fixture);
    let config = document(
        &fixture,
        &upstream,
        &records_table(&store, "call_bytes = 1024\n"),
    );
    let gateway = Gateway::start(&config, &fixture.dir);
    let skipped = gateway.counter("llmgw_interaction_records_skipped_total");
    let session = claude_session("over-bound");

    let answer = post(&gateway, "/v1/messages", &session, &first_turn("big"));
    assert_eq!(answer.status, 200);
    assert_eq!(String::from_utf8_lossy(&answer.body), big.body);
    let big_call = answer.call_id().expect("a call id");
    assert_eq!(
        gateway.counter("llmgw_interaction_records_skipped_total"),
        skipped + 1
    );

    // A call within the bound in the same session is stored; the big one never is.
    let answer = post(&gateway, "/v1/messages", &session, &first_turn("small"));
    assert_eq!(answer.status, 200);
    let steps = stored_when(
        &store,
        &session_hash("claude-code", "over-bound"),
        |steps| !turns(steps).is_empty(),
    );
    let turns = turns(&steps);
    assert_eq!(turns.len(), 1, "{:?}", names(&steps));
    assert_ne!(turns[0].call_id.as_deref(), Some(big_call.as_str()));
    assert_eq!(turns[0].message_id.as_deref(), Some("msg_small"));
}

// --- Acceptance 5: the session bound ------------------------------------------------------------

#[test]
fn a_session_past_sessions_held_continues_under_its_agent_id_in_the_store() {
    let fixture = Fixture::new("records-sessions-held");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    let config = document(
        &fixture,
        &upstream,
        &records_table(&store, "sessions_held = 2\n"),
    );
    let gateway = Gateway::start(&config, &fixture.dir);
    let conversation = |session: &str, turn: usize| {
        if turn == 0 {
            first_turn(&format!("hello {session}"))
        } else {
            request(&format!(
                r#"[{{"role":"user","content":"hello {session}"}},{{"role":"assistant","content":"ok"}},{{"role":"user","content":"again {session}"}}]"#
            ))
        }
    };
    // A, B, C fill the two held sessions with B and C; B is held, A was forgotten.
    for (session, turn) in [("a", 0), ("b", 0), ("c", 0), ("b", 1), ("a", 1)] {
        let answer = post(
            &gateway,
            "/v1/messages",
            &claude_session(session),
            &conversation(session, turn),
        );
        assert_eq!(answer.status, 200, "{session} {turn}");
    }
    let hash = |session: &str| session_hash("claude-code", session);
    let a = stored_when(&store, &hash("a"), |steps| turns(steps).len() == 2);
    let b = stored_when(&store, &hash("b"), |steps| turns(steps).len() == 2);
    let c = stored_when(&store, &hash("c"), |steps| turns(steps).len() == 1);
    // What the store shows: each session's agents by id. A's call after it was forgotten
    // continues under the agent id its first call started.
    let shown = |session: &str| -> Vec<String> {
        Store::open(&store)
            .unwrap()
            .agents(&hash(session))
            .unwrap()
            .into_iter()
            .map(|agent| agent.agent_id.0)
            .collect()
    };
    let first_agent = |steps: &[Step]| agents(steps)[0].agent_id.0.clone();
    assert_eq!(
        shown("a"),
        [first_agent(&a)],
        "A, forgotten: {:?}",
        names(&a)
    );
    assert!(
        agents(&a)
            .iter()
            .all(|start| start.agent_id.0 == first_agent(&a)),
        "every agent A's calls name is its first: {:?}",
        agents(&a)
    );
    assert_eq!(shown("b"), [first_agent(&b)], "B, held: {:?}", names(&b));
    assert_eq!(shown("c"), [first_agent(&c)], "{:?}", names(&c));
    assert_ne!(first_agent(&a), first_agent(&b));
}

// --- Acceptances 6 and 7: a two-agent session ---------------------------------------------------

const MAIN_PROMPT: &str = "PROMPT-MAIN-plan-the-feature";
const CHILD_PROMPT: &str = "PROMPT-CHILD-write-the-tests";
const SPAWN_DESCRIPTION: &str = "DESCRIPTION-of-the-spawned-task";
const CHILD_OUTPUT: &str = "OUTPUT-of-the-child-agent";
const TWO_AGENTS: &str = "two-agents";

/// What the two-agent session left: the store, the session's steps and each call's id.
struct TwoAgents {
    _fixture: Fixture,
    _gateway: Gateway,
    steps: Vec<Step>,
    main_call: String,
    count_call: Option<String>,
    child_call: String,
    refused_call: String,
    upstream_paths: Vec<String>,
}

/// The main agent asks for a `Task` whose prompt starts the child; a token count in the same
/// session; the child answers in a stream; then the main agent's next call names a model the
/// gateway does not serve and is refused.
fn two_agent_session(name: &str) -> TwoAgents {
    let fixture = Fixture::new(name);
    let spawn = format!(
        r#"{{"id":"msg_main_1","type":"message","role":"assistant","model":"claude","content":[{{"type":"tool_use","id":"toolu_spawn","name":"Task","input":{{"description":"{SPAWN_DESCRIPTION}","prompt":"{CHILD_PROMPT}"}}}}],"stop_reason":"tool_use","usage":{{"input_tokens":10,"output_tokens":5}}}}"#
    );
    let child = events(&[
        (
            "message_start",
            r#"{"type":"message_start","message":{"id":"msg_child_1","type":"message","role":"assistant","model":"claude","content":[],"stop_reason":null,"usage":{"input_tokens":7,"output_tokens":1}}}"#,
        ),
        (
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ),
        (
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"done"}}"#,
        ),
        (
            "content_block_stop",
            r#"{"type":"content_block_stop","index":0}"#,
        ),
        (
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#,
        ),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ]);
    let upstream = Upstream::start(vec![json(&spawn), child]);
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    let session = claude_session(TWO_AGENTS);

    let main = post(&gateway, "/v1/messages", &session, &first_turn(MAIN_PROMPT));
    assert_eq!(main.status, 200);
    let counted = post(
        &gateway,
        "/v1/messages/count_tokens",
        &session,
        &format!(
            r#"{{"model":"claude","messages":[{{"role":"user","content":"{MAIN_PROMPT}"}}]}}"#
        ),
    );
    assert_eq!(counted.status, 200);
    let child_body = format!(
        r#"{{"model":"claude","max_tokens":64,"stream":true,"tools":[{{"name":"Read","description":"read","input_schema":{{"type":"object"}}}}],"messages":[{{"role":"user","content":"{CHILD_PROMPT}"}}]}}"#
    );
    let child = post(&gateway, "/v1/messages", &session, &child_body);
    assert_eq!(child.status, 200);
    let refused_body = format!(
        r#"{{"model":"not-served","max_tokens":64,"tools":[{{"name":"Task","description":"spawn","input_schema":{{"type":"object"}}}}],"messages":[{{"role":"user","content":"{MAIN_PROMPT}"}},{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_spawn","name":"Task","input":{{"description":"{SPAWN_DESCRIPTION}","prompt":"{CHILD_PROMPT}"}}}}]}},{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_spawn","content":"{CHILD_OUTPUT}"}}]}}]}}"#
    );
    let refused = post(&gateway, "/v1/messages", &session, &refused_body);
    assert!(refused.status >= 400, "{}", refused.status);
    let refused_call = refused.call_id().expect("a refused call has a call id");

    let steps = stored_when(&store, &session_hash("claude-code", TWO_AGENTS), |steps| {
        turns(steps)
            .iter()
            .any(|turn| turn.call_id.as_deref() == Some(refused_call.as_str()))
    });
    TwoAgents {
        main_call: main.call_id().expect("a call id"),
        count_call: counted.call_id(),
        child_call: child.call_id().expect("a call id"),
        refused_call,
        upstream_paths: upstream.paths(),
        steps,
        _gateway: gateway,
        _fixture: fixture,
    }
}

#[test]
fn a_two_agent_session_and_a_refused_call_read_back_with_both_agents_the_link_and_the_facts() {
    let session = two_agent_session("records-two-agents");
    let steps = &session.steps;
    let started = agents(steps);
    assert_eq!(started.len(), 2, "{:?}", names(steps));
    let (main, child) = (started[0], started[1]);
    assert_eq!(format!("{:?}", main.role), "Main");
    assert_eq!(main.parent_agent_id, None);
    assert_eq!(format!("{:?}", child.role), "SubAgent");
    assert_eq!(child.parent_agent_id.as_ref(), Some(&main.agent_id));
    assert_eq!(
        child.spawned_by.as_ref().map(|call| call.0.as_str()),
        Some("toolu_spawn")
    );

    let requested: Vec<(&str, &str)> = steps
        .iter()
        .filter_map(|step| match &step.cmd {
            Cmd::RequestToolCall(call) => Some((call.tool_call_id.0.as_str(), call.tool.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(requested, [("toolu_spawn", "Task")]);
    let completed: Vec<&str> = steps
        .iter()
        .filter_map(|step| match &step.cmd {
            Cmd::CompleteToolCall(call) => Some(call.tool_call_id.0.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(completed, ["toolu_spawn"]);

    let turns = turns(steps);
    let calls: Vec<Option<&str>> = turns.iter().map(|turn| turn.call_id.as_deref()).collect();
    assert_eq!(
        calls,
        [
            Some(session.main_call.as_str()),
            Some(session.child_call.as_str()),
            Some(session.refused_call.as_str())
        ],
        "one model turn per model call, none for the token count"
    );
    let upstream = format!("{}", session.upstream_paths.len());
    assert!(
        session
            .upstream_paths
            .iter()
            .any(|path| path.ends_with("/count_tokens")),
        "the token count reached the endpoint ({upstream} requests)"
    );
    if let Some(count_call) = &session.count_call {
        assert!(
            turns
                .iter()
                .all(|turn| turn.call_id.as_deref() != Some(count_call.as_str()))
        );
    }
    for (turn, message, agent) in [
        (turns[0], "msg_main_1", &main.agent_id),
        (turns[1], "msg_child_1", &child.agent_id),
    ] {
        assert_eq!(turn.message_id.as_deref(), Some(message));
        assert_eq!(&turn.agent_id, agent);
        assert_eq!(turn.route.as_deref(), Some("claude"));
        assert!(turn.upstream.is_some(), "{message}: no upstream");
        assert_eq!(turn.attempts, Some(1), "{message}");
        assert!(turn.ttft_ms.is_some(), "{message}: no time to first token");
        assert_eq!(turn.incomplete, None, "{message}");
    }
    assert_eq!(
        format!("{:?}", turns[2].incomplete),
        "Some(Refused)",
        "the refused call is an incomplete turn"
    );
    assert_eq!(turns[2].agent_id, main.agent_id);
}

/// Every prompt, tool input and tool output the session sent, as the search looks for them.
fn sent_texts() -> Vec<String> {
    vec![
        MAIN_PROMPT.to_owned(),
        CHILD_PROMPT.to_owned(),
        SPAWN_DESCRIPTION.to_owned(),
        CHILD_OUTPUT.to_owned(),
        format!(r#"{{"description":"{SPAWN_DESCRIPTION}","prompt":"{CHILD_PROMPT}"}}"#),
    ]
}

fn found_in(texts: &[String]) -> Vec<String> {
    sent_texts()
        .into_iter()
        .filter(|sent| texts.iter().any(|text| text.contains(sent.as_str())))
        .collect()
}

#[test]
fn no_stored_command_holds_a_prompt_or_a_tool_input_and_a_planted_prompt_is_found() {
    let session = two_agent_session("records-no-text");
    let texts: Vec<String> = session
        .steps
        .iter()
        .map(|step| command_text(&step.cmd))
        .collect();
    assert!(texts.len() >= 10, "{:?}", names(&session.steps));
    assert_eq!(found_in(&texts), Vec::<String>::new());

    // The same search over one stored prompt whose digest is replaced by the prompt finds it.
    let mut planted = session
        .steps
        .iter()
        .find(|step| matches!(step.cmd, Cmd::SubmitPrompt(_)))
        .expect("a stored prompt")
        .cmd
        .clone();
    if let Cmd::SubmitPrompt(prompt) = &mut planted {
        prompt.content.digest = MAIN_PROMPT.to_owned();
    }
    assert_eq!(found_in(&[command_text(&planted)]), [MAIN_PROMPT]);
}

// --- Acceptance 8: the session hash -------------------------------------------------------------

#[test]
fn each_header_source_keys_the_store_by_sessionspecs_session_hash() {
    let fixture = Fixture::new("records-session-hash");
    let upstream = Upstream::start(Vec::new());
    let store = store_in(&fixture);
    let config = document(&fixture, &upstream, &records_table(&store, ""));
    let gateway = Gateway::start(&config, &fixture.dir);
    let cases = [
        (
            "x-claude-code-session-id: cc-one\r\n",
            "claude-code",
            "cc-one".to_owned(),
        ),
        (
            "session-id: sid-one\r\n",
            "session-id",
            "sid-one".to_owned(),
        ),
        (
            "session_id: sid-two\r\n",
            "session-id",
            "sid-two".to_owned(),
        ),
        (
            "x-opencode-session: oc-one\r\n",
            "session-id",
            "oc-one".to_owned(),
        ),
        (
            "user-agent: probe/1.0\r\n",
            "agent-address",
            "probe/1.0\u{0}127.0.0.1".to_owned(),
        ),
    ];
    for (n, (header, _, _)) in cases.iter().enumerate() {
        let answer = post(
            &gateway,
            "/v1/messages",
            header,
            &first_turn(&format!("hash {n}")),
        );
        assert_eq!(answer.status, 200, "{header}");
    }
    for (header, source, value) in &cases {
        let hash = session_hash(source, value);
        let steps = stored_when(&store, &hash, |steps| !turns(steps).is_empty());
        let opened = steps
            .iter()
            .find_map(|step| match &step.cmd {
                Cmd::OpenSession(open) => Some(open),
                _ => None,
            })
            .expect("the session is opened");
        assert_eq!(
            opened.session_hash.as_deref(),
            Some(hash.as_str()),
            "{header}"
        );
    }
}

// --- Acceptance 10: the guide ----------------------------------------------------------------------

#[test]
fn the_guide_names_the_table_its_keys_the_session_bound_and_how_sessionspec_reads_the_file() {
    let guide = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../website/docs/guides/record-agent-sessions.md"),
    )
    .expect("the guide page exists");
    for named in [
        "[interaction_records]",
        "`path`",
        "`queue`",
        "`call_bytes`",
        "`sessions_held`",
        "1024",
        "8 MiB",
        "4096",
        "same agent id",
        "llmgw_interaction_records_dropped_total",
        "llmgw_interaction_records_skipped_total",
        "llmgw_interaction_records_failed_total",
        "makes no record",
        "a file of its own",
        "schema version",
        "calls not stored at stop",
        "`queue` times `call_bytes`",
        "sessionspec serve",
    ] {
        assert!(guide.contains(named), "the guide does not name {named}");
    }
}
