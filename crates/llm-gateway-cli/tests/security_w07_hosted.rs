//! Security review of story:hosted-endpoints: integrity conformance checks of the hosted
//! endpoint's own contract (`spec/domains/upstream.yaml` `HostedEndpoint`,
//! `spec/domains/deployment.yaml` `HostedDeclaration` and the TLS client), measured from outside
//! the process and through the library's public surface.
//!
//! Every endpoint is a loopback fixture; no credential is real and nothing leaves the loopback
//! interface. `203.0.113.10` (TEST-NET-3) appears only in documents that are parsed, never served.

mod support;

use llm_gateway::{CredentialHeader, TargetBearer};
use llm_gateway_cli::{EndpointCredential, HostedEndpoint, parse};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};
use support::{
    ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET, api_key_keys, bearer_keys, hosted_document,
    hosted_table,
    tls_pod::{TestRoot, TlsPod},
};

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const EXIT: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
const ENDPOINT_ANSWER: &str = r#"{"id":"hosted-1","object":"chat.completion"}"#;
/// A second endpoint credential, so one endpoint can be checked for the other's.
const OTHER_CREDENTIAL: &str = "second-hosted-endpoint-credential-not-a-secret";
/// What a client puts in its own credential headers.
const CLIENT_KEY: &str = "client-supplied-key-must-not-be-forwarded";

fn shipped() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// --- Processes ----------------------------------------------------------------------------------

struct Server {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
    written: Vec<String>,
}

impl Server {
    fn start(program: &Path, config: &Path, env: &[(&str, &str)]) -> Self {
        let mut command = Command::new(program);
        command
            .args(["--config", config.to_str().unwrap()])
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
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
            written,
        }
    }

    fn stop(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.written.push(line);
        }
        self.written
    }
}

/// Runs the shipped binary on `config` to its exit; `Some(stderr)` when it refused, `None` when
/// it started listening (it is then stopped).
fn outcome(config: &Path) -> Option<String> {
    let mut child = Command::new(shipped())
        .args(["--config", config.to_str().unwrap()])
        .env_remove("RUST_LOG")
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
    let deadline = Instant::now() + EXIT;
    let mut text = String::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match lines.recv_timeout(left) {
            Ok(line) => {
                if line.starts_with(LISTENING) {
                    drop(child.kill());
                    drop(child.wait());
                    return None;
                }
                text.push_str(&line);
                text.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                drop(child.wait());
                return Some(text);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(child.kill());
                drop(child.wait());
                panic!("neither refused nor listened: {text:?}");
            }
        }
    }
}

/// The `<source>:<rule>` code of a `refused` line, its source dropped: `unsafe-mode`, `too-large`.
fn refused_rule(stderr: &str) -> Option<String> {
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{PREFIX}refused ")))?;
    let code = line.split(": ").next()?;
    Some(code.split_once(':')?.1.to_owned())
}

fn exchange(address: SocketAddr, request: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    answer
}

fn get(address: SocketAddr, path: &str, extra: &str) -> Vec<u8> {
    exchange(
        address,
        &format!(
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{extra}\r\n"
        ),
    )
}

fn post(address: SocketAddr, path: &str, body: &str, extra: &str) -> Vec<u8> {
    exchange(
        address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
}

// --- A plain `http` fixture endpoint -------------------------------------------------------------

#[derive(Debug, Clone)]
struct Received {
    head: String,
    raw: Vec<u8>,
}

impl Received {
    fn names(&self) -> Vec<String> {
        self.head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.trim().to_ascii_lowercase())
            .collect()
    }
}

/// How a fixture endpoint answers.
#[derive(Clone, Copy)]
enum Behaviour {
    Answer,
    Bad502,
    CloseUnanswered,
}

struct Endpoint {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Received>>>,
}

impl Endpoint {
    fn start(behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let Some(received) = read_request(&mut connection) else {
                    continue;
                };
                recording.lock().unwrap().push(received);
                let answer = match behaviour {
                    Behaviour::Answer => format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ENDPOINT_ANSWER}",
                        ENDPOINT_ANSWER.len()
                    ),
                    Behaviour::Bad502 => {
                        "HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            .to_owned()
                    }
                    Behaviour::CloseUnanswered => String::new(),
                };
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self { address, seen }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn seen(&self) -> Vec<Received> {
        self.seen.lock().unwrap().clone()
    }
}

fn read_request(connection: &mut TcpStream) -> Option<Received> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
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
    Some(Received { head, raw })
}

/// A loopback address nothing listens on.
fn closed_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

// --- Invariant 2: no client credential header, and no other endpoint's credential --------------

/// A client that sends its own `authorization`, `x-api-key`, `api-key`, `proxy-authorization`
/// and `cookie` headers reaches each hosted endpoint with none of them: each endpoint receives
/// exactly the head the gateway writes (its own credential header, if any, and the four framing
/// headers), never the owner's credential, the client's key, or the other endpoint's credential.
#[test]
fn security_w07_client_credential_headers_never_reach_an_endpoint() {
    let fixture = Fixture::new("security-w07-client-headers");
    let credential = fixture.endpoint_credential();
    let other = fixture.write(
        "other-credential",
        format!("{OTHER_CREDENTIAL}\n").as_bytes(),
        0o600,
    );
    let (bearer, keyed, open, second) = (
        Endpoint::start(Behaviour::Answer),
        Endpoint::start(Behaviour::Answer),
        Endpoint::start(Behaviour::Answer),
        Endpoint::start(Behaviour::Answer),
    );
    let tables = format!(
        "{}{}{}{}",
        hosted_table("remote", &bearer.base_url(), &bearer_keys(&credential)),
        hosted_table(
            "keyed",
            &keyed.base_url(),
            &api_key_keys(&credential, "x-api-key")
        ),
        hosted_table("open", &open.base_url(), "auth_kind = \"anonymous\"\n"),
        hosted_table("second", &second.base_url(), &bearer_keys(&other)),
    );
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ));
    let server = Server::start(&shipped(), &config, &[]);
    let extra = format!(
        "x-api-key: {CLIENT_KEY}\r\napi-key: {CLIENT_KEY}\r\nproxy-authorization: Bearer {CLIENT_KEY}\r\ncookie: session={CLIENT_KEY}\r\nx-goog-api-key: {CLIENT_KEY}\r\n"
    );
    for alias in ["remote", "keyed", "open", "second"] {
        for path in ["/v1/chat/completions", "/v1/messages"] {
            let answer = post(
                server.address,
                path,
                &format!(r#"{{"model":"{alias}","messages":[]}}"#),
                &extra,
            );
            assert!(
                answer.starts_with(b"HTTP/1.1 200 "),
                "{alias} {path}: {}",
                String::from_utf8_lossy(&answer)
            );
        }
    }
    server.stop();
    let framing = ["content-type", "content-length", "connection"];
    for (name, endpoint, credential_header, own, foreign) in [
        (
            "remote",
            &bearer,
            Some("authorization"),
            Some(ENDPOINT_CREDENTIAL),
            OTHER_CREDENTIAL,
        ),
        (
            "keyed",
            &keyed,
            Some("x-api-key"),
            Some(ENDPOINT_CREDENTIAL),
            OTHER_CREDENTIAL,
        ),
        ("open", &open, None, None, OTHER_CREDENTIAL),
        (
            "second",
            &second,
            Some("authorization"),
            Some(OTHER_CREDENTIAL),
            ENDPOINT_CREDENTIAL,
        ),
    ] {
        let seen = endpoint.seen();
        assert_eq!(seen.len(), 2, "{name}: {seen:?}");
        for request in &seen {
            let mut expected = vec!["host".to_owned()];
            expected.extend(credential_header.map(str::to_owned));
            expected.extend(framing.iter().map(|name| (*name).to_owned()));
            assert_eq!(request.names(), expected, "{name}: {request:?}");
            for absent in [CLIENT_KEY, OWNER_SECRET, foreign] {
                assert!(
                    !contains(&request.raw, absent.as_bytes()),
                    "{name} received {absent:?}: {request:?}"
                );
            }
            if let Some(own) = own {
                assert_eq!(
                    request
                        .raw
                        .windows(own.len())
                        .filter(|w| *w == own.as_bytes())
                        .count(),
                    1,
                    "{name}: its credential exactly once: {request:?}"
                );
            }
        }
    }
}

// --- Invariant 1: trace-level logs and every failure answer ------------------------------------

/// With `RUST_LOG=trace`, a document of a `bearer` endpoint that answers, an `api-key` endpoint
/// that answers, endpoints that refuse the connection, close unanswered and answer 502, and an
/// `https` endpoint whose certificate the shipped roots do not trust: no byte of either
/// credential appears in any answer or any standard-error line, and no endpoint's authority or
/// base URL appears in any answer a client receives, the refusals and inventories included.
#[test]
fn security_w07_trace_logs_and_failure_answers_withhold_credential_and_url() {
    let fixture = Fixture::new("security-w07-trace");
    let credential = fixture.endpoint_credential();
    let other = fixture.write(
        "other-credential",
        format!("{OTHER_CREDENTIAL}\n").as_bytes(),
        0o600,
    );
    let ok = Endpoint::start(Behaviour::Answer);
    let keyed = Endpoint::start(Behaviour::Answer);
    let bad = Endpoint::start(Behaviour::Bad502);
    let silent = Endpoint::start(Behaviour::CloseUnanswered);
    let closed = closed_address();
    let root = TestRoot::new("security w07 untrusted root");
    let pod = TlsPod::start(root.server("127.0.0.1"), None);
    let urls = [
        ("ok", ok.base_url()),
        ("keyed", keyed.base_url()),
        ("bad", bad.base_url()),
        ("silent", silent.base_url()),
        ("closed", format!("http://{closed}")),
        ("tls", format!("https://{}", pod.address)),
    ];
    let mut tables = String::new();
    for (alias, url) in &urls {
        let keys = match *alias {
            "keyed" => api_key_keys(&other, "x-api-key"),
            _ => bearer_keys(&credential),
        };
        tables.push_str(&hosted_table(alias, url, &keys));
    }
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ));
    let server = Server::start(&shipped(), &config, &[("RUST_LOG", "trace")]);
    let answers = trace_answers(server.address, &urls);
    let stderr = server.stop();
    assert_failures_are_upstream_failed(&answers);
    let tls_seen = pod.seen();
    assert!(
        tls_seen
            .iter()
            .all(|seen| !seen.handshake && seen.request.is_empty()),
        "{tls_seen:?}"
    );
    let mut withheld_from_clients: Vec<String> = vec![
        ENDPOINT_CREDENTIAL.to_owned(),
        OTHER_CREDENTIAL.to_owned(),
        credential.display().to_string(),
        other.display().to_string(),
    ];
    for (_, url) in &urls {
        withheld_from_clients.push(url.clone());
        withheld_from_clients.push(url.split("://").nth(1).unwrap().to_owned());
    }
    assert_no_answer_carries(&answers, &withheld_from_clients);
    assert_trace_names_no_secret(&stderr, &urls);
}

/// Every answer of the trace run: each model on each wire and its route, the owner's routes, an
/// unauthenticated request and an unknown model.
fn trace_answers(address: SocketAddr, urls: &[(&str, String)]) -> Vec<(String, Vec<u8>)> {
    let mut answers: Vec<(String, Vec<u8>)> = Vec::new();
    for (alias, _) in urls {
        for path in ["/v1/chat/completions", "/v1/messages", "/v1/responses"] {
            answers.push((
                format!("{alias} {path}"),
                post(
                    address,
                    path,
                    &format!(r#"{{"model":"{alias}","messages":[]}}"#),
                    "",
                ),
            ));
        }
        answers.push((
            format!("/v1/routes/{alias}"),
            get(address, &format!("/v1/routes/{alias}"), ""),
        ));
    }
    for path in [
        "/v1/routes",
        "/v1/models",
        "/",
        "/metrics",
        "/v1/routes/absent",
    ] {
        answers.push((path.to_owned(), get(address, path, "")));
    }
    // Unauthenticated, and an unknown model: refusal bodies.
    answers.push((
        "unauthenticated".to_owned(),
        exchange(
            address,
            "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\ncontent-length: 2\r\n\r\n{}",
        ),
    ));
    answers.push((
        "unknown model".to_owned(),
        post(
            address,
            "/v1/chat/completions",
            r#"{"model":"absent","messages":[]}"#,
            "",
        ),
    ));
    answers
}

/// The chat answers of the endpoints that fail are each `upstream-failed` (502).
fn assert_failures_are_upstream_failed(answers: &[(String, Vec<u8>)]) {
    let failures: Vec<&(String, Vec<u8>)> = answers
        .iter()
        .filter(|(case, _)| {
            ["bad ", "silent ", "closed ", "tls "]
                .iter()
                .any(|prefix| case.starts_with(prefix) && case.ends_with("completions"))
        })
        .collect();
    assert_eq!(failures.len(), 4);
    for (case, answer) in failures {
        assert!(
            answer.starts_with(b"HTTP/1.1 502 ") && contains(answer, b"upstream-failed"),
            "{case}: {}",
            String::from_utf8_lossy(answer)
        );
    }
}

/// No answer a client receives carries any of `withheld_from_clients`.
fn assert_no_answer_carries(answers: &[(String, Vec<u8>)], withheld_from_clients: &[String]) {
    for (case, answer) in answers {
        for withheld in withheld_from_clients {
            assert!(
                !contains(answer, withheld.as_bytes()),
                "{case}: {withheld:?} in the answer: {}",
                String::from_utf8_lossy(answer)
            );
        }
    }
}

/// Standard error at trace carries events above info and no credential; the authorities it names
/// are reported, not asserted.
fn assert_trace_names_no_secret(stderr: &[String], urls: &[(&str, String)]) {
    let joined = stderr.join("\n");
    assert!(
        stderr
            .iter()
            .any(|line| line.contains("TRACE") || line.contains("DEBUG")),
        "RUST_LOG=trace raised no event above info: {joined}"
    );
    for secret in [ENDPOINT_CREDENTIAL, OTHER_CREDENTIAL, OWNER_SECRET] {
        assert!(
            !joined.contains(secret),
            "{secret:?} in standard error at trace: {joined}"
        );
    }
    // Report, not assert: the base URL is only withheld from client-facing answers.
    let named: Vec<&str> = urls
        .iter()
        .map(|(_, url)| url.split("://").nth(1).unwrap())
        .filter(|authority| joined.contains(authority))
        .collect();
    eprintln!("authorities named on standard error at trace: {named:?}");
}

// --- Invariant 3: the credential header name ------------------------------------------------------

fn hosted_api_key_document(fixture: &Fixture, header_toml: &str) -> String {
    hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &format!(
            "[hosted.keyed]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9\"\nauth_kind = \"api-key\"\ncredential_file = \"{}\"\ncredential_header = {header_toml}\n",
            fixture.endpoint_credential().display()
        ),
    )
}

/// `credential_header` refuses, as `config:value`, every header the gateway writes and every
/// hop-by-hop or framing header, in any case, and every name that is not one RFC 9110 token
/// (a CR, LF, colon, space, NUL or non-ASCII byte, empty, over 64 bytes), through the document
/// parser and through `CredentialHeader::new`; a refusal never quotes the name. An ordinary
/// vendor header is admitted and held lowercase.
#[test]
fn security_w07_credential_header_cannot_name_a_reserved_header_or_inject_bytes() {
    let fixture = Fixture::new("security-w07-credential-header");
    let reserved = [
        "host",
        "Host",
        "HOST",
        "authorization",
        "Authorization",
        "content-type",
        "Content-Length",
        "connection",
        "Transfer-Encoding",
        "TE",
        "trailer",
        "Upgrade",
        "expect",
        "Keep-Alive",
        "proxy-authorization",
        "Proxy-Connection",
    ];
    let injecting = [
        "x-api-key\r\nx-injected: 1",
        "x-api-key\n",
        "x-api-key\r",
        "x-api-key:",
        "x-api-key: v",
        "x api key",
        " x-api-key",
        "x-api-key\u{0}",
        "x-äpi-key",
        "",
    ];
    let too_long = "k".repeat(65);
    for name in reserved
        .iter()
        .chain(injecting.iter())
        .copied()
        .chain([too_long.as_str()])
    {
        assert!(
            CredentialHeader::new(name).is_err(),
            "CredentialHeader::new admitted {name:?}"
        );
        // A TOML basic string carries every byte through its escapes.
        let toml_value = serde_json::to_string(name).unwrap();
        let document = hosted_api_key_document(&fixture, &toml_value);
        let refusal = parse(&document).expect_err(&format!("the document admitted {name:?}"));
        assert!(refusal.code().starts_with("config:"), "{name:?}: {refusal}");
        // A reserved name such as `host` is a substring of the key `hosted.keyed`; only the
        // injecting names are checked for being quoted.
        if injecting.contains(&name) && !name.is_empty() {
            assert!(
                !refusal.message().contains(name),
                "{name:?}: the refusal quotes the name: {refusal}"
            );
        }
    }
    for (name, held) in [
        ("x-api-key", "x-api-key"),
        ("X-Goog-Api-Key", "x-goog-api-key"),
        ("api-key", "api-key"),
    ] {
        assert_eq!(CredentialHeader::new(name).unwrap().as_str(), held);
        let document = hosted_api_key_document(&fixture, &serde_json::to_string(name).unwrap());
        let deployment = parse(&document).unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
        assert_eq!(
            deployment.hosted["keyed"]
                .credential_header
                .as_ref()
                .map(CredentialHeader::as_str),
            Some(held)
        );
    }
}

// --- Invariant 4: no document key reaches the TLS client ---------------------------------------

/// No key in a `[hosted.<alias>]` table or at the top of the document adds a trust root, names a
/// certificate, or weakens verification: each is refused `config:schema`.
#[test]
fn security_w07_no_document_key_reaches_a_trust_root_or_disables_verification() {
    let fixture = Fixture::new("security-w07-trust-keys");
    let credential = fixture.endpoint_credential();
    let root = fixture.write("root.pem", b"-----BEGIN CERTIFICATE-----\n", 0o600);
    let path = root.display().to_string();
    for key in [
        format!("ca_file = \"{path}\""),
        format!("root_certificate = \"{path}\""),
        format!("trust_roots = [\"{path}\"]"),
        format!("certificate = \"{path}\""),
        "tls_verify = false".to_owned(),
        "verify = false".to_owned(),
        "insecure = true".to_owned(),
        "danger_accept_invalid_certs = true".to_owned(),
        "tls = { verify = false }".to_owned(),
        "server_name = \"localhost\"".to_owned(),
    ] {
        let table = hosted_table(
            "remote",
            "https://api.example.test",
            &format!("{}{key}\n", bearer_keys(&credential)),
        );
        let document = hosted_document("127.0.0.1:0", &fixture.owner_secret(), &table);
        let refusal = parse(&document).expect_err(&format!("the table admitted {key}"));
        assert_eq!(refusal.code(), "config:schema", "{key}: {refusal}");
        let top = format!(
            "{key}\n{}",
            hosted_document(
                "127.0.0.1:0",
                &fixture.owner_secret(),
                &hosted_table(
                    "remote",
                    "https://api.example.test",
                    &bearer_keys(&credential)
                )
            )
        );
        let refusal = parse(&top).expect_err(&format!("the document admitted {key}"));
        assert_eq!(
            refusal.code(),
            "config:schema",
            "top-level {key}: {refusal}"
        );
    }
}

/// The judgement finding on cleartext, closed: a `bearer` or `api-key` endpoint at a
/// non-loopback `http://` base URL is refused at load as `config:value`, so its credential is
/// never written in cleartext. The refusal names the endpoint and quotes no credential.
#[test]
fn security_w07_a_non_loopback_http_endpoint_with_a_credential_is_refused() {
    let fixture = Fixture::new("security-w07-cleartext");
    let credential = fixture.endpoint_credential();
    for (url, keys) in [
        ("http://203.0.113.10", bearer_keys(&credential)),
        (
            "http://api.example.test:8080/",
            api_key_keys(&credential, "x-api-key"),
        ),
    ] {
        let document = hosted_document(
            "127.0.0.1:0",
            &fixture.owner_secret(),
            &hosted_table("remote", url, &keys),
        );
        let refusal = parse(&document).expect_err(url);
        assert_eq!(refusal.code(), "config:value", "{url}: {refusal}");
        assert!(
            refusal.message().contains("hosted.remote"),
            "{url}: {refusal}"
        );
        assert!(
            !refusal.message().contains(ENDPOINT_CREDENTIAL),
            "{url}: {refusal}"
        );
    }
}

/// A hosted endpoint's `Debug`, the only formatting it has, shows no credential byte.
#[test]
fn security_w07_a_hosted_endpoint_debug_shows_no_credential() {
    for credential in [
        EndpointCredential::Bearer(
            TargetBearer::new(ENDPOINT_CREDENTIAL.as_bytes().to_vec()).unwrap(),
        ),
        EndpointCredential::ApiKey {
            header: CredentialHeader::new("x-api-key").unwrap(),
            key: TargetBearer::new(ENDPOINT_CREDENTIAL.as_bytes().to_vec()).unwrap(),
        },
    ] {
        let endpoint = HostedEndpoint::new("https://api.example.test", credential).unwrap();
        let shown = format!("{endpoint:?} {endpoint:#?}");
        assert!(!shown.contains(ENDPOINT_CREDENTIAL), "{shown}");
    }
}

// --- Invariant 5: the credential file follows the owner secret's rules, rule for rule -----------

/// Each file shape below is given once as `owner_secret_file` (beside a valid credential file)
/// and once as a `bearer` endpoint's `credential_file` (beside a valid owner secret). Both
/// outcomes must agree: both start, or both are refused with the same rule after the source
/// (`owner-secret:<rule>` and `endpoint-credential:<rule>`). The material is longer than the
/// owner secret's own minimum, the one rule only an owner secret has.
#[test]
fn security_w07_credential_file_rules_match_the_owner_secret_rules() {
    let fixture = Fixture::new("security-w07-differential");
    let token = "differential-credential-0123456789abcdefghijklmnop";
    let valid_owner = fixture.owner_secret();
    let valid_credential = fixture.endpoint_credential();
    let mut cases: Vec<(String, PathBuf)> = Vec::new();
    let mut file = |name: &str, bytes: &[u8], mode: u32| {
        let path = fixture.write(name, bytes, mode);
        cases.push((name.to_owned(), path));
    };
    let token_line = format!("{token}\n");
    for mode in [
        0o600, 0o400, 0o700, 0o610, 0o601, 0o640, 0o604, 0o660, 0o606, 0o620, 0o602,
    ] {
        file(&format!("mode-{mode:03o}"), token_line.as_bytes(), mode);
    }
    file("crlf", format!("{token}\r\n").as_bytes(), 0o600);
    file(
        "trailing-spaces",
        format!("{token} \t \n\n").as_bytes(),
        0o600,
    );
    file("leading-space", format!(" {token}\n").as_bytes(), 0o600);
    file(
        "inner-space",
        format!("{token} {token}\n").as_bytes(),
        0o600,
    );
    file("inner-cr", format!("{token}\r{token}\n").as_bytes(), 0o600);
    file(
        "inner-crlf",
        format!("{token}\r\nx-injected: 1\n").as_bytes(),
        0o600,
    );
    file("nul", format!("{token}\0\n").as_bytes(), 0o600);
    file("inner-nul", format!("{token}\0{token}").as_bytes(), 0o600);
    file("bom", format!("\u{feff}{token}\n").as_bytes(), 0o600);
    file("non-ascii", format!("{token}é\n").as_bytes(), 0o600);
    file("not-utf8", b"credential-\xff\xfe\n", 0o600);
    file("empty", b"", 0o600);
    file("whitespace-only", b" \r\n\t\n", 0o600);
    file("token-4096", "c".repeat(4096).as_bytes(), 0o600);
    file(
        "token-4096-crlf",
        format!("{}\r\n", "c".repeat(4096)).as_bytes(),
        0o600,
    );
    file("token-4097", "c".repeat(4097).as_bytes(), 0o600);
    file(
        "file-4099",
        format!("{}\r\n\n", "c".repeat(4096)).as_bytes(),
        0o600,
    );
    let link = fixture.path("symlink");
    symlink(&valid_credential, &link).unwrap();
    cases.push(("symlink".to_owned(), link));
    cases.push(("directory".to_owned(), fixture.dir.clone()));
    cases.push(("absent".to_owned(), fixture.path("absent")));
    let fifo = fixture.path("fifo");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    cases.push(("fifo".to_owned(), fifo));

    let mut disagreements = Vec::new();
    for (case, path) in &cases {
        let as_owner = fixture.config(&hosted_document(
            "127.0.0.1:0",
            path,
            &hosted_table(
                "remote",
                "http://127.0.0.1:9",
                &bearer_keys(&valid_credential),
            ),
        ));
        let owner = outcome(&as_owner);
        let as_credential = fixture.config(&hosted_document(
            "127.0.0.1:0",
            &valid_owner,
            &hosted_table("remote", "http://127.0.0.1:9", &bearer_keys(path)),
        ));
        let endpoint = outcome(&as_credential);
        let owner_rule = owner.as_deref().map(refused_rule);
        let endpoint_rule = endpoint.as_deref().map(refused_rule);
        if let Some(stderr) = &endpoint {
            assert!(
                stderr.contains("refused endpoint-credential:"),
                "{case}: {stderr:?}"
            );
            assert!(
                !stderr.contains(token),
                "{case}: quotes the credential: {stderr:?}"
            );
        }
        if owner_rule != endpoint_rule {
            disagreements.push(format!(
                "{case}: owner-secret {owner_rule:?}, endpoint-credential {endpoint_rule:?}"
            ));
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}
