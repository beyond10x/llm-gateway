//! `b10x-llm-gateway credential set` (`spec/domains/deployment.yaml` `CredentialSetCommand`):
//! the owner stores one upstream key in the file the deployment already names for it.
//!
//! Every case drives the binary with `--stdin`. Every endpoint is a loopback fixture: a plain
//! `http` server on 127.0.0.1 that records each request and answers a fixed status, or the
//! `https` fixture pod of `support::tls_pod` with a certificate a test root issued for
//! `127.0.0.1`. No test reaches any other address, and no key is real.

mod support;

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use support::{
    ENDPOINT_CREDENTIAL, Fixture, api_key_keys, bearer_keys, document, hosted_document,
    hosted_table,
    tls_pod::{TestRoot, TlsPod},
};

/// The key every case stores. Never written anywhere but the target and the probe.
const KEY: &str = "sk-test-0123456789-upstream-key-not-a-secret";
const STORED: &str = "b10x-llm-gateway: credential stored ";
const STARTUP: Duration = Duration::from_secs(20);

fn shipped() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

fn seam() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gateway-tls-seam"))
}

// --- Fixture endpoints --------------------------------------------------------------------------

/// A loopback `http` endpoint answering every request with `status` and recording its head.
struct Endpoint {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Endpoint {
    fn start(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                connection
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut chunk = [0_u8; 4096];
                while !raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => raw.extend_from_slice(&chunk[..read]),
                    }
                }
                recording
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).into_owned());
                let body = r#"{"object":"list","data":[]}"#;
                drop(write!(
                    connection,
                    "HTTP/1.1 {status} Fixture\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                ));
            }
        });
        Self { address, seen }
    }

    fn base(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// A loopback address nothing listens on.
fn closed_port() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    address
}

/// The header `name`'s value in a recorded request head, its name matched without case.
fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.split("\r\n").skip(1).find_map(|line| {
        let (field, value) = line.split_once(':')?;
        field
            .eq_ignore_ascii_case(name)
            .then_some(value.trim_start())
    })
}

// --- The command ----------------------------------------------------------------------------------

struct Finished {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// `program credential set --config <config> <args> --stdin`, `input` on standard input, `HOME`
/// at `home`. Every run is checked: neither stream carries the key or any 12-byte part of it.
fn run_with(program: &Path, config: &Path, args: &[&str], input: &[u8], home: &Path) -> Finished {
    run_raw(
        program,
        &[
            &["credential", "set", "--config", config.to_str().unwrap()][..],
            args,
            &["--stdin"],
        ]
        .concat(),
        input,
        home,
        &[],
    )
}

fn run_raw(
    program: &Path,
    args: &[&str],
    input: &[u8],
    home: &Path,
    env: &[(&str, &Path)],
) -> Finished {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let input = input.to_vec();
    let writer = thread::spawn(move || drop(stdin.write_all(&input)));
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    let finished = Finished {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    for start in 0..=KEY.len() - 12 {
        let part = &KEY[start..start + 12];
        assert!(
            !finished.stdout.contains(part) && !finished.stderr.contains(part),
            "a stream carries part of the key: {:?} {:?}",
            finished.stdout,
            finished.stderr
        );
    }
    finished
}

fn run(fixture: &Fixture, config: &Path, args: &[&str], input: &[u8]) -> Finished {
    run_with(&shipped(), config, args, input, &fixture.dir)
}

fn line(key: &str) -> Vec<u8> {
    format!("{key}\n").into_bytes()
}

fn assert_refused(finished: &Finished, code: &str) {
    assert_eq!(finished.code, Some(1), "{code}: {:?}", finished.stderr);
    let prefix = format!("b10x-llm-gateway: refused {code}: ");
    assert!(
        finished.stderr.starts_with(&prefix) && finished.stderr.lines().count() == 1,
        "expected one line {prefix:?}…, got {:?}",
        finished.stderr
    );
}

fn assert_stored(finished: &Finished, path: &Path, probe: &str, http: &str) {
    assert_eq!(finished.code, Some(0), "{:?}", finished.stderr);
    assert_eq!(
        finished.stderr,
        format!("{STORED}{} probe {probe} http {http}\n", path.display())
    );
    assert_eq!(fs::read(path).unwrap(), KEY.as_bytes(), "exactly the key");
    let mode = fs::symlink_metadata(path).unwrap().permissions().mode();
    assert_eq!(mode & 0o7777, 0o600, "{mode:o}");
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_file());
}

/// The names in `dir`, sorted: a temporary file left behind shows here.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A fixture holding a `keys` directory (0700) for the target, and a document whose hosted
/// model `remote` is a bearer endpoint at `base_url` storing its key at `keys/remote`.
struct Setup {
    fixture: Fixture,
    keys: PathBuf,
    target: PathBuf,
}

impl Setup {
    fn new(name: &str) -> Self {
        let fixture = Fixture::new(&format!("credential-{name}"));
        let keys = fixture.path("keys");
        fs::create_dir(&keys).unwrap();
        fs::set_permissions(&keys, fs::Permissions::from_mode(0o700)).unwrap();
        let target = keys.join("remote");
        Self {
            fixture,
            keys,
            target,
        }
    }

    /// A document with `tables`, its owner secret written.
    fn config(&self, tables: &str) -> PathBuf {
        let secret = self.fixture.owner_secret();
        self.fixture
            .config(&hosted_document("127.0.0.1:0", &secret, tables))
    }

    fn bearer(&self, base_url: &str) -> PathBuf {
        self.config(&hosted_table(
            "remote",
            base_url,
            &bearer_keys(&self.target),
        ))
    }
}

// --- Criterion 2: a 200 probe stores a 0600 file the gateway then starts with -----------------

#[test]
fn credential_set_a_passed_probe_stores_a_0600_file_the_gateway_then_starts_with() {
    let setup = Setup::new("stores");
    let endpoint = Endpoint::start(200);
    let config = setup.bearer(&endpoint.base("/v1"));
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_stored(&finished, &setup.target, "passed", "200");
    assert_eq!(finished.stdout, "");
    assert_eq!(names(&setup.keys), ["remote"], "no temporary file is left");
    let requests = endpoint.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("GET /v1/models HTTP/1.1\r\n"),
        "{requests:?}"
    );
    assert_eq!(
        header(&requests[0], "authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );

    // The gateway starts with the stored key: it reads the file through its own reader.
    let mut child = Command::new(shipped())
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let mut text = Vec::new();
        let mut byte = [0_u8; 1];
        while let Ok(1) = stderr.read(&mut byte) {
            text.push(byte[0]);
            if byte[0] == b'\n' {
                drop(sender.send(String::from_utf8_lossy(&text).into_owned()));
                text.clear();
            }
        }
    });
    let deadline = Instant::now() + STARTUP;
    let mut lines = Vec::new();
    let listening = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok(line) if line.starts_with("b10x-llm-gateway: listening on ") => break true,
            Ok(line) => lines.push(line),
            Err(_) => break false,
        }
    };
    drop(child.kill());
    drop(child.wait());
    assert!(listening, "the gateway did not start: {lines:?}");
}

#[test]
fn credential_set_a_key_without_a_terminator_and_a_key_of_4096_bytes_are_stored() {
    let setup = Setup::new("terminator");
    let config = setup.bearer("http://127.0.0.1:9/v1");
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        KEY.as_bytes(),
    );
    assert_stored(&finished, &setup.target, "skipped", "-");

    let longest = "k".repeat(4096);
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(&longest),
    );
    assert_eq!(finished.code, Some(0), "{:?}", finished.stderr);
    assert_eq!(fs::read(&setup.target).unwrap(), longest.as_bytes());
}

// --- Criterion 2: 401 and 403 leave the target unchanged ----------------------------------------

#[test]
fn credential_set_a_rejected_probe_leaves_the_target_unchanged_and_no_temporary_file() {
    for status in [401, 403] {
        let setup = Setup::new(&format!("rejected-{status}"));
        let endpoint = Endpoint::start(status);
        let config = setup.bearer(&endpoint.base("/v1"));
        let before = format!("{ENDPOINT_CREDENTIAL}\n");
        fs::write(&setup.target, &before).unwrap();
        fs::set_permissions(&setup.target, fs::Permissions::from_mode(0o600)).unwrap();
        let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
        assert_refused(&finished, "credential:probe-rejected");
        assert!(
            finished.stderr.contains(&status.to_string()),
            "{:?}",
            finished.stderr
        );
        assert_eq!(
            fs::read(&setup.target).unwrap(),
            before.as_bytes(),
            "{status}"
        );
        assert_eq!(names(&setup.keys), ["remote"], "{status}");
        assert_eq!(endpoint.requests().len(), 1);
    }
}

#[test]
fn credential_set_any_other_status_stores_and_names_its_code() {
    let setup = Setup::new("other-status");
    let endpoint = Endpoint::start(404);
    let config = setup.bearer(&endpoint.base("/v1"));
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_stored(&finished, &setup.target, "not-rejected", "404");
}

// --- Criterion 2: a transport failure refuses -------------------------------------------------

#[test]
fn credential_set_a_transport_failure_refuses_and_writes_nothing() {
    let setup = Setup::new("transport");
    let config = setup.bearer(&format!("http://{}/v1", closed_port()));
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_refused(&finished, "credential:probe-unreachable");
    assert!(names(&setup.keys).is_empty());

    // `--no-probe` stores anyway.
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_stored(&finished, &setup.target, "skipped", "-");
}

// --- Criterion 2: --no-probe sends no request ---------------------------------------------------

#[test]
fn credential_set_no_probe_sends_no_request() {
    let setup = Setup::new("no-probe");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let config = setup.bearer(&format!("http://{}/v1", listener.local_addr().unwrap()));
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_stored(&finished, &setup.target, "skipped", "-");
    // A connection the process opened would wait in the backlog after it exited.
    assert_eq!(
        listener.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "--no-probe opened a connection"
    );
}

// --- Criterion 2: the probe's request -------------------------------------------------------------

#[test]
fn credential_set_the_probe_carries_the_credential_header_and_static_headers_at_the_base_path() {
    let setup = Setup::new("probe-headers");
    let endpoint = Endpoint::start(200);
    let config = setup.config(&format!(
        "{}[hosted.remote.headers]\nanthropic-version = \"2023-06-01\"\nX-Title = \"b10x gateway\"\n",
        hosted_table(
            "remote",
            &endpoint.base("/api/v1"),
            &api_key_keys(&setup.target, "x-api-key"),
        )
    ));
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_stored(&finished, &setup.target, "passed", "200");
    let requests = endpoint.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let request = &requests[0];
    assert!(
        request.starts_with("GET /api/v1/models HTTP/1.1\r\n"),
        "{request:?}"
    );
    assert_eq!(header(request, "x-api-key"), Some(KEY));
    assert_eq!(header(request, "authorization"), None, "{request:?}");
    assert_eq!(header(request, "anthropic-version"), Some("2023-06-01"));
    assert_eq!(header(request, "x-title"), Some("b10x gateway"));
    assert_eq!(
        header(request, "host"),
        Some(endpoint.address.to_string().as_str())
    );

    // A base URL without a path probes `/v1/models`, the path the gateway relays a request for
    // `/v1/models` to when the endpoint has no base path.
    let setup = Setup::new("probe-root");
    let endpoint = Endpoint::start(200);
    let config = setup.bearer(&endpoint.base(""));
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_stored(&finished, &setup.target, "passed", "200");
    assert!(
        endpoint.requests()[0].starts_with("GET /v1/models HTTP/1.1\r\n"),
        "{:?}",
        endpoint.requests()
    );
}

#[test]
fn credential_set_an_https_probe_goes_over_the_gateways_tls_client() {
    let root = TestRoot::new("credential-set test root");
    let pod = TlsPod::start(root.server("127.0.0.1"), None);
    let setup = Setup::new("https");
    let root_file = setup.fixture.write("root.der", &root.der, 0o600);
    let config = setup.bearer(&format!("https://127.0.0.1:{}/v1", pod.address.port()));

    // The shipped binary trusts only its compiled-in roots: the handshake fails before any
    // request byte, and nothing is stored.
    let finished = run(&setup.fixture, &config, &["--model", "remote"], &line(KEY));
    assert_refused(&finished, "credential:probe-unreachable");
    assert!(names(&setup.keys).is_empty());
    assert!(pod.seen().iter().all(|seen| seen.request.is_empty()));

    // The same client over the test root verifies the certificate and probes.
    let finished = run_raw(
        &seam(),
        &[
            "credential",
            "set",
            "--config",
            config.to_str().unwrap(),
            "--model",
            "remote",
            "--stdin",
        ],
        &line(KEY),
        &setup.fixture.dir,
        &[("GATEWAY_TLS_SEAM_ROOT", &root_file)],
    );
    assert_stored(&finished, &setup.target, "passed", "200");
    let probed: Vec<String> = pod
        .seen()
        .iter()
        .filter(|seen| !seen.request.is_empty())
        .map(support::tls_pod::Seen::text)
        .collect();
    assert_eq!(probed.len(), 1, "{probed:?}");
    assert!(
        probed[0].starts_with("GET /v1/models HTTP/1.1\r\n"),
        "{probed:?}"
    );
    assert_eq!(
        header(&probed[0], "authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
}

// --- Criterion 2: every input rule refuses --------------------------------------------------------

#[test]
fn credential_set_every_input_rule_refuses() {
    let setup = Setup::new("input");
    let config = setup.bearer("http://127.0.0.1:9/v1");
    let too_large = "k".repeat(4097);
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (b"".to_vec(), "credential:input-empty"),
        (b"\n".to_vec(), "credential:input-empty"),
        (
            format!("{KEY}\r\n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            format!("{KEY}\r").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            format!(" {KEY}\n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            format!("{KEY} \n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            format!("{KEY}\t\n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (b"sk-one two\n".to_vec(), "credential:input-not-a-token"),
        (
            format!("{KEY}\n\n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            format!("{KEY}\nsecond\n").into_bytes(),
            "credential:input-not-a-token",
        ),
        (
            "sk-caf\u{e9}-key\n".as_bytes().to_vec(),
            "credential:input-not-a-token",
        ),
        (b"sk-\x01-key\n".to_vec(), "credential:input-not-a-token"),
        (b"sk-\xff-key\n".to_vec(), "credential:input-not-a-token"),
        (line(&too_large), "credential:input-too-large"),
        (too_large.clone().into_bytes(), "credential:input-too-large"),
    ];
    for (input, code) in cases {
        let finished = run(
            &setup.fixture,
            &config,
            &["--model", "remote", "--no-probe"],
            &input,
        );
        assert_refused(&finished, code);
        assert!(names(&setup.keys).is_empty(), "{input:?}");
    }
}

#[test]
fn credential_set_takes_no_key_from_an_argument() {
    let setup = Setup::new("argument");
    let config = setup.bearer("http://127.0.0.1:9/v1");
    let config = config.to_str().unwrap();
    for args in [
        &[
            "credential",
            "set",
            "--config",
            config,
            "--model",
            "remote",
            KEY,
        ][..],
        &[
            "credential",
            "set",
            "--config",
            config,
            "--model",
            "remote",
            "--key",
            KEY,
        ],
        &[
            "credential",
            "set",
            "--config",
            config,
            "--model",
            "remote",
            "--value",
            KEY,
        ],
    ] {
        let mut command = Command::new(shipped());
        let output = command.args(args).stdin(Stdio::null()).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
    assert!(names(&setup.keys).is_empty());
}

// --- The target rule ----------------------------------------------------------------------------

#[test]
fn credential_set_refuses_a_target_it_cannot_store_a_key_for() {
    let setup = Setup::new("targets");
    let secret = setup.fixture.owner_secret();
    let runpod = document("127.0.0.1:0", &secret);
    let anonymous = hosted_table(
        "open",
        "http://127.0.0.1:9/v1",
        "auth_kind = \"anonymous\"\n",
    );
    let remote = hosted_table(
        "remote",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&setup.target),
    );
    let config = setup
        .fixture
        .config(&format!("{runpod}\n{anonymous}\n{remote}"));
    for (args, code) in [
        (&["--model", "absent"][..], "credential:unknown-target"),
        (&["--route", "absent"], "credential:unknown-target"),
        (&["--model", "small"], "credential:not-hosted"),
        (&["--model", "open"], "credential:anonymous"),
    ] {
        let finished = run(&setup.fixture, &config, args, &line(KEY));
        assert_refused(&finished, code);
    }
    // Exactly one of `--model` and `--route`.
    for args in [
        &[
            "credential",
            "set",
            "--config",
            config.to_str().unwrap(),
            "--stdin",
        ][..],
        &[
            "credential",
            "set",
            "--config",
            config.to_str().unwrap(),
            "--model",
            "remote",
            "--route",
            "remote",
            "--stdin",
        ],
    ] {
        let finished = run_raw(&shipped(), args, &line(KEY), &setup.fixture.dir, &[]);
        assert_eq!(finished.code, Some(2), "{args:?}: {:?}", finished.stderr);
    }
    assert!(names(&setup.keys).is_empty());
}

/// Loading the target reads the document only: an owner secret that does not exist, another
/// endpoint's missing credential and a target the server would refuse are never opened.
#[test]
fn credential_set_loading_the_target_reads_no_credential_or_owner_secret_file() {
    let setup = Setup::new("reads-nothing");
    let missing = setup.fixture.path("no-such-owner-secret");
    let other = hosted_table(
        "other",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&setup.fixture.path("no-such-credential")),
    );
    let remote = hosted_table(
        "remote",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&setup.target),
    );
    let config = setup.fixture.config(&hosted_document(
        "127.0.0.1:0",
        &missing,
        &format!("{other}\n{remote}"),
    ));
    // The target holds what the server would refuse: world-readable, not a token.
    fs::write(&setup.target, "not a token at all\n").unwrap();
    fs::set_permissions(&setup.target, fs::Permissions::from_mode(0o644)).unwrap();
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_stored(&finished, &setup.target, "skipped", "-");
    // A document the server would not load is refused with the server's code.
    let broken = setup
        .fixture
        .config("listen = \"127.0.0.1:0\"\nunknown = 1\n");
    let finished = run(&setup.fixture, &broken, &["--model", "remote"], &line(KEY));
    assert_refused(&finished, "config:schema");
}

// --- Criterion 2: a --route target resolves its file: path -----------------------------------------

#[test]
fn credential_set_a_route_target_resolves_its_file_path_below_home() {
    let setup = Setup::new("route");
    let endpoint = Endpoint::start(200);
    let routes = setup.fixture.write(
        "routes.yaml",
        format!(
            "format: llm.routes/1
endpoints:
  claude:
    base_url: {}
    protocol: messages
    auth: {{kind: api-key, header: x-api-key, secret: \"file:~/keys/remote\"}}
routes:
  opus: claude/claude-opus-4-1
",
            endpoint.base("/anthropic/v1")
        )
        .as_bytes(),
        0o600,
    );
    let secret = setup.fixture.owner_secret();
    let config = setup.fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\nroutes = \"{}\"\n",
        secret.display(),
        routes.display()
    ));
    let finished = run(&setup.fixture, &config, &["--route", "opus"], &line(KEY));
    // `HOME` is the fixture directory, so `~/keys/remote` is the setup's target.
    assert_stored(&finished, &setup.target, "passed", "200");
    let requests = endpoint.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].starts_with("GET /anthropic/v1/models HTTP/1.1\r\n"),
        "{requests:?}"
    );
    assert_eq!(header(&requests[0], "x-api-key"), Some(KEY));
    assert_eq!(
        header(&requests[0], "anthropic-version"),
        Some("2023-06-01")
    );
    // A route is not a `--model` target.
    let finished = run(&setup.fixture, &config, &["--model", "opus"], &line(KEY));
    assert_refused(&finished, "credential:unknown-target");
}

/// A refusal never quotes the unknown `--model` or `--route` value, whatever its shape: a
/// generated key looks like a plain short name. It is called "the given name".
#[test]
fn credential_set_an_unknown_target_name_is_never_quoted() {
    let setup = Setup::new("quote-target");
    let secret = setup.fixture.owner_secret();
    let remote = hosted_table(
        "remote",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&setup.target),
    );
    let config = setup
        .fixture
        .config(&format!("{}\n{remote}", document("127.0.0.1:0", &secret)));
    let long = "a".repeat(65);
    let longest = "a".repeat(64);
    for flag in ["--model", "--route"] {
        for name in [
            "plain-name_1.2",
            longest.as_str(),
            long.as_str(),
            "has space",
            "sk-looks/like:a-key",
            "quote\"d",
            "ünï",
        ] {
            let finished = run(&setup.fixture, &config, &[flag, name], &line(KEY));
            assert_refused(&finished, "credential:unknown-target");
            assert!(
                !finished.stderr.contains(name),
                "{flag} {name:?}: {:?}",
                finished.stderr
            );
            assert!(
                finished.stderr.contains("the given name"),
                "{:?}",
                finished.stderr
            );
        }
    }
}

// --- Criterion 3: a write the reader would refuse is refused and leaves no file ---------------------

#[test]
fn credential_set_a_directory_another_user_could_write_is_refused_and_left_without_a_file() {
    let tmp = Path::new("/tmp");
    let mode = fs::metadata(tmp).unwrap().permissions().mode();
    assert_ne!(mode & 0o002, 0, "/tmp is world-writable on the test host");
    let setup = Setup::new("unsafe-directory");
    let name = format!("b10x-credential-set-{}", std::process::id());
    let target = tmp.join(&name);
    drop(fs::remove_file(&target));
    let config = setup.config(&hosted_table(
        "remote",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&target),
    ));
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_refused(&finished, "credential:directory-unsafe");
    assert!(!target.exists());
    assert!(
        names(tmp).iter().all(|entry| !entry.contains(&name)),
        "a temporary file is left in /tmp"
    );

    // A directory that does not exist.
    let config = setup.config(&hosted_table(
        "remote",
        "http://127.0.0.1:9/v1",
        &bearer_keys(&setup.keys.join("absent").join("remote")),
    ));
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_refused(&finished, "credential:directory-missing");
}

#[test]
fn credential_set_replaces_a_symlink_without_writing_through_it_and_refuses_a_directory() {
    let setup = Setup::new("write");
    let config = setup.bearer("http://127.0.0.1:9/v1");
    let elsewhere = setup.fixture.write("elsewhere", b"untouched\n", 0o600);
    symlink(&elsewhere, &setup.target).unwrap();
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_stored(&finished, &setup.target, "skipped", "-");
    assert_eq!(fs::read(&elsewhere).unwrap(), b"untouched\n");

    // A target that is a directory cannot be replaced: refused, and no temporary file is left.
    fs::remove_file(&setup.target).unwrap();
    fs::create_dir(&setup.target).unwrap();
    let finished = run(
        &setup.fixture,
        &config,
        &["--model", "remote", "--no-probe"],
        &line(KEY),
    );
    assert_refused(&finished, "credential:write");
    assert_eq!(names(&setup.keys), ["remote"]);
    assert!(fs::metadata(&setup.target).unwrap().is_dir());
}

// --- Criterion 4: the serving command line is unchanged -------------------------------------------

#[test]
fn credential_set_has_its_own_help_and_the_serving_line_is_unchanged() {
    let help = Command::new(shipped())
        .args(["credential", "set", "--help"])
        .output()
        .unwrap();
    assert_eq!(help.status.code(), Some(0));
    let text = String::from_utf8_lossy(&help.stdout);
    for expected in [
        "--config <CONFIG>",
        "--model <ALIAS>",
        "--route <NAME>",
        "--stdin",
        "--no-probe",
    ] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    // `credential` after `--config` is still an unexpected argument of the serving line.
    let setup = Setup::new("serving-line");
    let config = setup.bearer("http://127.0.0.1:9/v1");
    let output = Command::new(shipped())
        .args(["--config", config.to_str().unwrap(), "credential"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}
