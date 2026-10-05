//! Adversary cases for story:gateway-binary (rows C1, K29, K30, D2). Each case names the row it
//! attacks. Red cases assert what the matrix, the specification or the README promise; green
//! cases pin a rule no other test in this crate would catch a mutation of.

mod support;

use llm_gateway_cli::load;
use rustix::process::{Pid, Signal, kill_process};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, SECRET_LIMIT, document, mutate};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const EXIT: Duration = Duration::from_secs(20);

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_b10x-llm-gateway")
}

/// Polls `child` until it exits or `within` passes; `None` means it is still running.
fn exit_within(child: &mut Child, within: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() > deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn kill(mut child: Child) {
    drop(child.kill());
    drop(child.wait());
}

/// Runs `program args` to its exit with standard error captured.
fn run(program: &str, args: &[&str]) -> (ExitStatus, String) {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        let mut text = String::new();
        drop(stderr.read_to_string(&mut text));
        text
    });
    let Some(status) = exit_within(&mut child, EXIT) else {
        kill(child);
        panic!("{program} {args:?} was still running after {EXIT:?}");
    };
    (status, reader.join().unwrap())
}

fn assert_refused(status: ExitStatus, stderr: &str, code: &str) {
    let line = format!("b10x-llm-gateway: refused {code}: ");
    assert!(
        stderr.lines().any(|candidate| candidate.starts_with(&line)),
        "expected a line starting {line:?}; stderr: {stderr:?}"
    );
    assert_eq!(status.code(), Some(1), "stderr: {stderr:?}");
}

fn run_config(config: &Path) -> (ExitStatus, String) {
    run(binary(), &["--config", config.to_str().unwrap()])
}

/// Spawns a server and returns it with its address once it reported listening. The returned
/// reader is standard error after the listening line.
#[allow(
    clippy::zombie_processes,
    reason = "the child is returned to the caller, which kills and waits on it"
)]
fn serve(config: &Path) -> (Child, SocketAddr, BufReader<std::process::ChildStderr>) {
    let mut child = Command::new(binary())
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    let mut seen = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            let status = child.wait().unwrap();
            panic!("b10x-llm-gateway never reported listening ({status}); stderr: {seen:?}");
        }
        seen.push_str(&line);
        if let Some(address) = line.trim_end().strip_prefix(LISTENING) {
            return (child, address.parse().unwrap(), reader);
        }
    }
}

fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(status.success(), "mkfifo {}", path.display());
}

/// Whether this host lets an unprivileged process map itself to another uid in a new user
/// namespace (`unshare --user --map-user`).
fn user_namespaces() -> bool {
    Command::new("unshare")
        .args(["--user", "--map-user=4242", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

// --- K30: the trusted-file reader -----------------------------------------------------------

/// A FIFO at the config path: `O_NONBLOCK` is what keeps the open from blocking until a writer
/// appears. Nothing else in the suite opens a FIFO, so dropping the flag stays green there.
#[test]
fn adv_k30_a_fifo_config_or_owner_secret_is_refused_without_blocking() {
    let fixture = Fixture::new("adv-k30-fifo");
    let fifo = fixture.path("gateway.fifo");
    mkfifo(&fifo);
    let (status, stderr) = run_config(&fifo);
    assert_refused(status, &stderr, "config:not-regular");

    let secret_fifo = fixture.path("secret.fifo");
    mkfifo(&secret_fifo);
    let config = fixture.config(&document("127.0.0.1:0", &secret_fifo));
    let (status, stderr) = run_config(&config);
    assert_refused(status, &stderr, "owner-secret:not-regular");
}

/// The matrix says the owner rule "has no test, because it needs a file owned by another user".
/// A user namespace that maps this user to uid 4242 makes every root-owned file appear owned by
/// the overflow uid, which is neither the effective user nor root.
#[test]
fn adv_k30_a_file_owned_by_neither_this_user_nor_root_is_refused() {
    if !user_namespaces() {
        eprintln!("SKIPPED: this host refuses unprivileged user namespaces");
        return;
    }
    let root_owned = "/etc/passwd";
    // Control, outside the namespace: root is trusted, so the reader admits the file and the
    // document parser is what refuses it.
    let (status, stderr) = run(binary(), &["--config", root_owned]);
    assert_refused(status, &stderr, "config:schema");

    let (status, stderr) = run(
        "unshare",
        &[
            "--user",
            "--map-user=4242",
            binary(),
            "--config",
            root_owned,
        ],
    );
    assert_refused(status, &stderr, "config:untrusted-owner");

    let fixture = Fixture::new("adv-k30-owner");
    let config = fixture.config(&document("127.0.0.1:0", Path::new(root_owned)));
    let (status, stderr) = run(
        "unshare",
        &[
            "--user",
            "--map-user=4242",
            binary(),
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert_refused(status, &stderr, "owner-secret:untrusted-owner");
}

/// README: "The owner secret must be one printable token of 32 to 4096 bytes", and the
/// specification trims trailing whitespace before the token rules. A 4096-byte token written
/// with the newline every shell tool adds is 4097 bytes on disk and is refused as too large.
#[test]
fn adv_k30_a_4096_byte_owner_secret_with_its_trailing_newline_is_served() {
    let fixture = Fixture::new("adv-k30-secret-4096");
    let config = fixture.valid_config("127.0.0.1:0");
    fixture.write(
        "owner-secret",
        format!("{}\n", "s".repeat(SECRET_LIMIT)).as_bytes(),
        0o600,
    );
    let (mut child, address, _stderr) = serve(&config);
    assert!(TcpStream::connect(address).is_ok());
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let status = exit_within(&mut child, EXIT);
    kill(child);
    assert_eq!(status.and_then(|status| status.code()), Some(0));
}

/// A secret file written on another platform ends in CRLF; the trailing-whitespace trim must
/// take both bytes, and a NUL is not whitespace.
#[test]
fn adv_k30_an_owner_secret_ending_in_crlf_is_served_and_one_ending_in_nul_is_refused() {
    let fixture = Fixture::new("adv-k30-secret-crlf");
    let config = fixture.valid_config("127.0.0.1:0");
    fixture.write(
        "owner-secret",
        format!("{OWNER_SECRET}\0").as_bytes(),
        0o600,
    );
    let (status, stderr) = run_config(&config);
    assert_refused(status, &stderr, "owner-secret:not-a-token");

    fixture.write(
        "owner-secret",
        format!("{OWNER_SECRET}\r\n").as_bytes(),
        0o600,
    );
    let (child, address, _stderr) = serve(&config);
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .write_all(
            format!(
                "GET /v1/routes HTTP/1.1\r\nHost: g\r\nAuthorization: Bearer {OWNER_SECRET}\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    kill(child);
    assert!(response.starts_with("HTTP/1.1 200 "), "{response:?}");
}

// --- K29: the closed TOML document -----------------------------------------------------------

/// `config.rs` says its `gpu_util` range check "is false for NaN as well"; no case passes NaN,
/// so a rewrite to `!(gpu_util < 0.1 || gpu_util > 1.0)` would stay green.
#[test]
fn adv_k29_a_nan_or_infinite_gpu_util_is_refused_as_a_value() {
    let fixture = Fixture::new("adv-k29-nan");
    let text = document("127.0.0.1:0", &fixture.owner_secret());
    for value in ["nan", "+inf", "-inf"] {
        let config = fixture.config(&mutate(
            &text,
            "max_model_len = 1024\n",
            &format!("max_model_len = 1024\ngpu_util = {value}\n"),
        ));
        match load(&config) {
            Ok(_) => panic!("gpu_util = {value}: accepted"),
            Err(refusal) => assert_eq!(refusal.code(), "config:value", "gpu_util = {value}"),
        }
    }
}

/// Duplicate keys and tables, a value past the field's integer width, and multi-byte text at
/// the error position (the refusal slices the document at the error's byte offset).
#[test]
fn adv_k29_duplicates_overflow_and_multibyte_errors_are_schema_refusals() {
    let fixture = Fixture::new("adv-k29-schema");
    let text = document("127.0.0.1:0", &fixture.owner_secret());
    let cases: &[(&str, &str, &str)] = &[
        (
            "a duplicate key",
            "max_model_len = 1024\n",
            "max_model_len = 1024\nmax_model_len = 2048\n",
        ),
        (
            "a duplicate table",
            "[models.small]\n",
            "[models.small]\nprovider = \"runpod\"\n[models.small]\n",
        ),
        (
            "a context_window past u32",
            "context_window = 65536",
            "context_window = 4294967296",
        ),
        (
            "a multi-byte bare key",
            "max_model_len = 1024\n",
            "max_model_len = 1024\n\u{e9}\u{e9} = 1\n",
        ),
        (
            "a multi-byte value",
            "context_window = 65536",
            "context_window = \u{1f600}",
        ),
        (
            "an unterminated multi-byte string",
            "hf_model = \"example/small-model\"",
            "hf_model = \"\u{e9}\u{e9}\u{e9}",
        ),
    ];
    for (case, find, replace) in cases {
        let config = fixture.config(&mutate(&text, find, replace));
        let (status, stderr) = run_config(&config);
        assert!(
            stderr.contains("refused config:schema: "),
            "{case}: {stderr:?}"
        );
        assert_eq!(status.code(), Some(1), "{case}: {stderr:?}");
    }
}

// --- C1 / D2: exit statuses and the stop ------------------------------------------------------

/// The specification: a refused start exits 1. With standard error a broken pipe (a log reader
/// that went away), `eprintln!` panics and the process exits 101.
#[test]
fn adv_c1_a_refused_start_exits_1_when_standard_error_is_a_broken_pipe() {
    let fixture = Fixture::new("adv-c1-epipe");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let mut child = Command::new(binary())
        .args(["--config", fixture.path("absent.toml").to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn()
        .unwrap();
    let status = exit_within(&mut child, EXIT);
    kill(child);
    assert_eq!(
        status.and_then(|status| status.code()),
        Some(1),
        "config:unreadable is exit status 1"
    );
}

/// D2: "a request in flight is answered, then the process exits 0". After the log reader on
/// standard error goes away, a SIGTERM still drains and stops, but writing the stop line panics
/// and the process exits 101, which a supervisor records as a failed unit.
#[test]
fn adv_d2_a_signalled_stop_exits_0_after_standard_error_breaks() {
    let fixture = Fixture::new("adv-d2-epipe");
    let config = fixture.valid_config("127.0.0.1:0");
    let (mut child, _address, stderr) = serve(&config);
    drop(stderr);
    // A child another test is spawning at this moment may briefly share the read end.
    thread::sleep(Duration::from_millis(300));
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let status = exit_within(&mut child, EXIT);
    kill(child);
    assert_eq!(status.and_then(|status| status.code()), Some(0));
}

/// D2 with a client that trickles a request head one byte a second. The gateway's read timeout
/// (10 s) applies per read, so the head is never complete and never timed out, and `shutdown`
/// joins that connection: SIGTERM does not stop the process. A second signal is swallowed by the
/// still-registered handler, so only SIGKILL ends it.
#[test]
fn adv_d2_sigterm_stops_within_twice_the_read_timeout_despite_a_trickling_client() {
    let fixture = Fixture::new("adv-d2-trickle");
    let config = fixture.valid_config("127.0.0.1:0");
    let (mut child, address, _stderr) = serve(&config);
    let mut stream = TcpStream::connect(address).unwrap();
    stream.write_all(b"GET /health HTTP/1.1\r\n").unwrap();
    let trickle = thread::spawn(move || {
        for _ in 0..40 {
            thread::sleep(Duration::from_secs(1));
            if stream.write_all(b"X").is_err() {
                return;
            }
        }
    });
    thread::sleep(Duration::from_millis(300));
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    thread::sleep(Duration::from_secs(1));
    kill_process(Pid::from_child(&child), Signal::TERM).unwrap();
    let status = exit_within(&mut child, Duration::from_secs(20));
    kill(child);
    drop(trickle.join());
    assert!(
        status.is_some(),
        "b10x-llm-gateway was still running 20 s after two SIGTERMs"
    );
}
