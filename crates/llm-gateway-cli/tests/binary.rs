//! The `b10x-llm-gateway` process, observed from outside: its command line, its refusals and its
//! stop. Each test is named after the docs/llmgw-capability-matrix.md row it closes (C1, C2, C4,
//! C5, K29, K30, D2). Only loopback sockets and files under `CARGO_TARGET_TMPDIR` are used.
//!
//! Every refusal case has a positive control in the same file that serves from the same
//! document with the one property changed back, so a refusal can be shown to fail.

mod support;

use rustix::process::{Pid, Signal, kill_process};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};
use support::{CONFIG_LIMIT, Fixture, OWNER_SECRET, SECRET_LIMIT, document, mutate, padded};

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const EXIT: Duration = Duration::from_secs(20);
/// Long enough for the gateway's 2 ms accept poll and for a signal to be delivered and acted on.
const SETTLE: Duration = Duration::from_millis(300);

fn binary() -> PathBuf {
    let Some(path) = option_env!("CARGO_BIN_EXE_b10x-llm-gateway") else {
        panic!("row C1: the workspace builds no `b10x-llm-gateway` binary");
    };
    PathBuf::from(path)
}

struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn drain(mut reader: impl Read + Send + 'static) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut text = String::new();
        drop(reader.read_to_string(&mut text));
        text
    })
}

/// Runs the binary to its exit. A process still running after [`EXIT`] is killed and the case
/// fails: it served where it should have refused.
fn run(args: &[&str]) -> Finished {
    let mut child = Command::new(binary())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = drain(child.stdout.take().unwrap());
    let stderr = drain(child.stderr.take().unwrap());
    let deadline = Instant::now() + EXIT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            drop(child.kill());
            drop(child.wait());
            panic!(
                "b10x-llm-gateway {args:?} was still running after {EXIT:?}; stderr: {}",
                stderr.join().unwrap()
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    Finished {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    }
}

fn run_config(config: &Path) -> Finished {
    run(&["--config", config.to_str().unwrap()])
}

/// Asserts a startup refusal: exit status 1 and the `<source>:<rule>` code on standard error.
fn assert_refused(finished: &Finished, code: &str) {
    let line = format!("{PREFIX}refused {code}: ");
    assert!(
        finished
            .stderr
            .lines()
            .any(|candidate| candidate.starts_with(&line)),
        "expected a line starting {line:?}; stderr: {:?}",
        finished.stderr
    );
    assert_eq!(
        finished.status.code(),
        Some(1),
        "a startup refusal exits 1; stderr: {:?}",
        finished.stderr
    );
}

/// A served process, read from its standard error line by line.
struct Server {
    child: Option<Child>,
    address: SocketAddr,
    lines: Receiver<String>,
}

impl Server {
    fn start(config: &Path) -> Self {
        let mut child = Command::new(binary())
            .args(["--config", config.to_str().unwrap()])
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
        let mut seen = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some(address) = line.strip_prefix(LISTENING) {
                        break address.parse::<SocketAddr>().unwrap_or_else(|error| {
                            panic!("the listening line names no address ({error}): {line:?}")
                        });
                    }
                    seen.push(line);
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    drop(child.kill());
                    let status = child.wait().unwrap();
                    panic!(
                        "b10x-llm-gateway never reported {LISTENING:?} (exit {status}); \
                         stderr: {seen:?}"
                    );
                }
            }
        };
        Self {
            child: Some(child),
            address,
            lines,
        }
    }

    fn signal(&self, signal: Signal) {
        let child = self.child.as_ref().unwrap();
        kill_process(Pid::from_child(child), signal).unwrap();
    }

    /// Waits for the process to exit by itself and returns its status and remaining stderr.
    fn wait(mut self) -> (ExitStatus, Vec<String>) {
        let mut child = self.child.take().unwrap();
        let deadline = Instant::now() + EXIT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                drop(child.kill());
                drop(child.wait());
                panic!("b10x-llm-gateway did not stop within {EXIT:?} of the signal");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let mut rest = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            rest.push(line);
        }
        (status, rest)
    }

    /// Stops the server with SIGTERM and asserts the clean exit.
    fn stop(self) {
        self.signal(Signal::TERM);
        let (status, lines) = self.wait();
        assert_eq!(status.code(), Some(0), "stderr after the stop: {lines:?}");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(child.kill());
            drop(child.wait());
        }
    }
}

fn exchange(address: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn get(address: SocketAddr, path: &str, bearer: Option<&str>) -> String {
    let authorization = bearer.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    exchange(
        address,
        &format!("GET {path} HTTP/1.1\r\nHost: gateway\r\n{authorization}\r\n"),
    )
}

fn status(response: &str) -> &str {
    response.split(' ').nth(1).unwrap_or_default()
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

/// A loopback address nothing is listening on at the moment of the call.
fn free_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

// --- C1: one binary, no subcommand; it reads config, refuses or serves -------------------------

#[test]
fn c1_one_binary_serves_the_configured_gateway() {
    let fixture = Fixture::new("c1-serves");
    let config = fixture.valid_config("127.0.0.1:0");
    let server = Server::start(&config);

    let health = get(server.address, "/health", None);
    assert_eq!(status(&health), "200", "{health:?}");
    let ready = get(server.address, "/ready", None);
    assert_eq!(
        status(&ready),
        "200",
        "the binary marks the gateway ready: {ready:?}"
    );

    // The owner verifier is the secret the document names, and nothing else.
    let anonymous = get(server.address, "/v1/routes/small", None);
    assert_eq!(status(&anonymous), "401", "{anonymous:?}");
    let wrong = get(
        server.address,
        "/v1/routes/small",
        Some("not-the-owner-secret-0123456789abcdef"),
    );
    assert_eq!(status(&wrong), "401", "{wrong:?}");

    // The configured model is served as a route, one target per declared wire, under the digest
    // of the exact document bytes.
    let route = get(server.address, "/v1/routes/small", Some(OWNER_SECRET));
    assert_eq!(status(&route), "200", "{route:?}");
    let digest = hex(&Sha256::digest(std::fs::read(&config).unwrap()));
    let route_body = body(&route);
    for expected in [
        format!("\"config_digest\":\"{digest}\""),
        "\"alias\":\"small\"".to_string(),
        "\"target_count\":2".to_string(),
    ] {
        assert!(
            route_body.contains(&expected),
            "the route body lacks {expected}: {route_body}"
        );
    }
    let unknown = get(server.address, "/v1/routes/large", Some(OWNER_SECRET));
    assert_eq!(status(&unknown), "404", "{unknown:?}");

    server.stop();
}

#[test]
fn c1_a_subcommand_is_refused() {
    let fixture = Fixture::new("c1-subcommand");
    let config = fixture.valid_config("127.0.0.1:0");
    let finished = run(&["serve", "--config", config.to_str().unwrap()]);
    assert_eq!(finished.status.code(), Some(2), "{:?}", finished.stderr);
    assert!(
        finished.stderr.contains("unexpected argument 'serve'"),
        "{:?}",
        finished.stderr
    );
}

#[test]
fn c1_a_listen_address_in_use_is_refused() {
    let fixture = Fixture::new("c1-bind");
    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = fixture.valid_config(&holder.local_addr().unwrap().to_string());
    assert_refused(&run_config(&config), "listen:bind");
    drop(holder);
}

// --- C2: --config <CONFIG> ----------------------------------------------------------------------

#[test]
fn c2_config_flag_loads_a_closed_toml() {
    let fixture = Fixture::new("c2-loads");
    let address = free_address();
    let config = fixture.valid_config(&address.to_string());
    let server = Server::start(&config);
    assert_eq!(
        server.address, address,
        "the gateway listens where the document says"
    );
    let health = get(address, "/health", None);
    assert_eq!(status(&health), "200", "{health:?}");
    server.stop();
}

#[test]
fn c2_the_config_flag_is_required() {
    let finished = run(&[]);
    assert_eq!(finished.status.code(), Some(2), "{:?}", finished.stderr);
    assert!(
        finished.stderr.contains("--config <CONFIG>"),
        "{:?}",
        finished.stderr
    );
}

// --- C4: --help, -h -----------------------------------------------------------------------------

#[test]
fn c4_help_lists_exactly_the_closed_command_line() {
    for flag in ["--help", "-h"] {
        let finished = run(&[flag]);
        assert_eq!(
            finished.status.code(),
            Some(0),
            "{flag}: {:?}",
            finished.stderr
        );
        let help = &finished.stdout;
        for expected in ["--config <CONFIG>", "-h, --help", "-V, --version"] {
            assert!(help.contains(expected), "{flag} lacks {expected:?}: {help}");
        }
        for absent in ["--insecure", "Commands:"] {
            assert!(!help.contains(absent), "{flag} offers {absent:?}: {help}");
        }
    }
}

// --- C5: --version, -V --------------------------------------------------------------------------

#[test]
fn c5_version_prints_the_binary_name_and_package_version() {
    for flag in ["--version", "-V"] {
        let finished = run(&[flag]);
        assert_eq!(
            finished.status.code(),
            Some(0),
            "{flag}: {:?}",
            finished.stderr
        );
        assert_eq!(
            finished.stdout,
            format!("b10x-llm-gateway {}\n", env!("CARGO_PKG_VERSION")),
            "{flag}"
        );
    }
}

// --- K29: the closed TOML document --------------------------------------------------------------

#[test]
fn k29_an_unknown_key_is_refused() {
    let fixture = Fixture::new("k29-unknown");
    let secret = fixture.owner_secret();
    let text = mutate(
        &document("127.0.0.1:0", &secret),
        "listen = ",
        "listn = \"127.0.0.1:0\"\nlisten = ",
    );
    let finished = run_config(&fixture.config(&text));
    assert_refused(&finished, "config:schema");
    assert!(finished.stderr.contains("listn"), "{:?}", finished.stderr);
}

#[test]
fn k29_a_value_outside_its_rule_is_refused() {
    let fixture = Fixture::new("k29-value");
    let secret = fixture.owner_secret();
    let text = mutate(
        &document("127.0.0.1:0", &secret),
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_seconds = 9\n",
    );
    let finished = run_config(&fixture.config(&text));
    assert_refused(&finished, "config:value");
    assert!(
        finished.stderr.contains("start_wait_seconds"),
        "{:?}",
        finished.stderr
    );
}

#[test]
fn k29_a_config_of_exactly_256_kib_is_read() {
    let fixture = Fixture::new("k29-at-limit");
    let secret = fixture.owner_secret();
    let config = fixture.config(&padded(&document("127.0.0.1:0", &secret), CONFIG_LIMIT));
    Server::start(&config).stop();
}

#[test]
fn k29_a_config_one_byte_over_256_kib_is_refused() {
    let fixture = Fixture::new("k29-over-limit");
    let secret = fixture.owner_secret();
    let config = fixture.config(&padded(&document("127.0.0.1:0", &secret), CONFIG_LIMIT + 1));
    assert_refused(&run_config(&config), "config:too-large");
}

// --- K30: the trusted-file reader ---------------------------------------------------------------

#[test]
fn k30_a_missing_config_is_refused() {
    let fixture = Fixture::new("k30-missing");
    assert_refused(
        &run_config(&fixture.path("absent.toml")),
        "config:unreadable",
    );
}

#[test]
fn k30_a_symlinked_config_is_refused() {
    let fixture = Fixture::new("k30-symlink");
    let target = fixture.valid_config("127.0.0.1:0");
    let link = fixture.path("link.toml");
    symlink(&target, &link).unwrap();
    assert_refused(&run_config(&link), "config:symlink");
    // Control: the file the link points at is served.
    Server::start(&target).stop();
}

#[test]
fn k30_a_config_that_is_not_a_regular_file_is_refused() {
    let fixture = Fixture::new("k30-directory");
    assert_refused(&run_config(&fixture.dir), "config:not-regular");
}

#[test]
fn k30_a_world_writable_config_is_refused() {
    let fixture = Fixture::new("k30-world-writable");
    let config = fixture.valid_config("127.0.0.1:0");
    let text = std::fs::read(&config).unwrap();
    for mode in [0o602, 0o620, 0o666] {
        let config = fixture.write("gateway.toml", &text, mode);
        assert_refused(&run_config(&config), "config:unsafe-mode");
    }
}

#[test]
fn k30_a_world_readable_config_is_read() {
    // llmgw refuses a writable config, not a readable one (src/trusted.rs:30, mode & 0o022): a
    // root-installed 0644 document is the hosted case.
    let fixture = Fixture::new("k30-world-readable");
    let config = fixture.valid_config("127.0.0.1:0");
    let text = std::fs::read(&config).unwrap();
    let config = fixture.write("gateway.toml", &text, 0o644);
    Server::start(&config).stop();
}

#[test]
fn k30_a_config_that_is_not_utf8_is_refused() {
    let fixture = Fixture::new("k30-not-utf8");
    let secret = fixture.owner_secret();
    let mut bytes = document("127.0.0.1:0", &secret).into_bytes();
    bytes.extend_from_slice(b"# \xff\xfe\n");
    let config = fixture.write("gateway.toml", &bytes, 0o600);
    assert_refused(&run_config(&config), "config:not-utf8");
}

#[test]
fn k30_an_owner_secret_readable_by_others_is_refused() {
    let fixture = Fixture::new("k30-secret-mode");
    let config = fixture.valid_config("127.0.0.1:0");
    for mode in [0o640, 0o604, 0o644] {
        fixture.write("owner-secret", format!("{OWNER_SECRET}\n").as_bytes(), mode);
        assert_refused(&run_config(&config), "owner-secret:unsafe-mode");
    }
    // Control: the same secret at 0600 is served.
    fixture.write(
        "owner-secret",
        format!("{OWNER_SECRET}\n").as_bytes(),
        0o600,
    );
    Server::start(&config).stop();
}

#[test]
fn k30_a_symlinked_owner_secret_is_refused() {
    let fixture = Fixture::new("k30-secret-symlink");
    let real = fixture.owner_secret();
    let link = fixture.path("secret-link");
    symlink(&real, &link).unwrap();
    let config = fixture.config(&document("127.0.0.1:0", &link));
    assert_refused(&run_config(&config), "owner-secret:symlink");
}

#[test]
fn k30_an_owner_secret_under_32_bytes_is_refused() {
    let fixture = Fixture::new("k30-secret-short");
    let config = fixture.valid_config("127.0.0.1:0");
    fixture.write(
        "owner-secret",
        format!("{}\n", "s".repeat(31)).as_bytes(),
        0o600,
    );
    assert_refused(&run_config(&config), "owner-secret:too-short");
    // Control: exactly 32 bytes is the verifier's minimum and is served.
    fixture.write(
        "owner-secret",
        format!("{}\n", "s".repeat(32)).as_bytes(),
        0o600,
    );
    Server::start(&config).stop();
}

#[test]
fn k30_an_owner_secret_that_is_not_one_token_is_refused() {
    let fixture = Fixture::new("k30-secret-token");
    let config = fixture.valid_config("127.0.0.1:0");
    for material in ["\n".to_string(), format!("{OWNER_SECRET} {OWNER_SECRET}\n")] {
        fixture.write("owner-secret", material.as_bytes(), 0o600);
        assert_refused(&run_config(&config), "owner-secret:not-a-token");
    }
}

#[test]
fn k30_an_owner_secret_over_4_kib_is_refused() {
    let fixture = Fixture::new("k30-secret-large");
    let config = fixture.valid_config("127.0.0.1:0");
    fixture.write(
        "owner-secret",
        "s".repeat(SECRET_LIMIT + 1).as_bytes(),
        0o600,
    );
    assert_refused(&run_config(&config), "owner-secret:too-large");
}

// --- D2: graceful shutdown on SIGINT and SIGTERM ------------------------------------------------

#[test]
fn d2_sigterm_stops_the_server() {
    let fixture = Fixture::new("d2-sigterm");
    let config = fixture.valid_config("127.0.0.1:0");
    let server = Server::start(&config);
    let address = server.address;

    // A request in flight when the signal arrives: its head is incomplete, so the gateway has
    // accepted the connection and is still reading it.
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: gateway\r\n")
        .unwrap();
    thread::sleep(SETTLE);
    server.signal(Signal::TERM);
    thread::sleep(SETTLE);
    stream.write_all(b"\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert_eq!(
        status(&response),
        "200",
        "the request in flight at SIGTERM is answered: {response:?}"
    );

    let (exit, lines) = server.wait();
    assert_eq!(exit.code(), Some(0), "a signalled stop exits 0: {lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line == "b10x-llm-gateway: stopped by SIGTERM accepted=1 completed=1"),
        "{lines:?}"
    );
    assert!(
        TcpStream::connect(address).is_err(),
        "the listener is closed after the stop"
    );
}

#[test]
fn d2_sigint_stops_the_server() {
    let fixture = Fixture::new("d2-sigint");
    let config = fixture.valid_config("127.0.0.1:0");
    let server = Server::start(&config);
    server.signal(Signal::INT);
    let (exit, lines) = server.wait();
    assert_eq!(exit.code(), Some(0), "a signalled stop exits 0: {lines:?}");
    assert!(
        lines
            .iter()
            .any(|line| line == "b10x-llm-gateway: stopped by SIGINT accepted=0 completed=0"),
        "{lines:?}"
    );
}
