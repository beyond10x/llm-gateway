//! story:hosted-endpoints: a relayed model served by a hosted endpoint declared in the
//! deployment document (`[hosted.<alias>]`, `spec/domains/deployment.yaml` `HostedDeclaration`
//! and `spec/domains/upstream.yaml` `HostedEndpoint`), observed from outside the process.
//!
//! Every endpoint is a loopback fixture: a plain `http` server recording each request, or the
//! `https` fixture pod of `support::tls_pod` with a certificate a test root issued for
//! `127.0.0.1`. The `https` cases run `gateway-tls-seam`, the shipped `main` with that test root
//! as its only trust root, and the shipped `b10x-llm-gateway` itself, which trusts only the roots
//! compiled into it. Nothing leaves the loopback interface and no credential is real.

mod support;

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
    ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET, SECRET_LIMIT, api_key_keys, bearer_keys, document,
    hosted_document, hosted_table,
    tls_pod::{POD_CHAT_ANSWER, TestRoot, TlsPod},
};

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const EXIT: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
/// What every plain fixture endpoint answers.
const ENDPOINT_ANSWER: &str = r#"{"id":"hosted-1","object":"chat.completion"}"#;

fn shipped() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

fn seam() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gateway-tls-seam"))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// --- Processes ----------------------------------------------------------------------------------

/// A served process, read from its standard error line by line; every line is kept.
struct Server {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
    written: Vec<String>,
}

impl Server {
    fn start(program: &Path, config: &Path, root: Option<&Path>) -> Self {
        let mut command = Command::new(program);
        command
            .args(["--config", config.to_str().unwrap()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(root) = root {
            command.env("GATEWAY_TLS_SEAM_ROOT", root);
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
                panic!(
                    "{} never reported {LISTENING:?}; stderr: {written:?}",
                    program.display()
                );
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

    /// Stops the process and returns every line it wrote to standard error.
    fn stop(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.written.push(line);
        }
        self.written
    }
}

/// Runs `program` to its exit and returns its exit code and standard error.
fn refused(program: &Path, config: &Path) -> (Option<i32>, String) {
    let mut child = Command::new(program)
        .args(["--config", config.to_str().unwrap()])
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
    let deadline = Instant::now() + EXIT;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            drop(child.kill());
            drop(child.wait());
            panic!("the gateway served where it should have refused");
        }
        thread::sleep(Duration::from_millis(10));
    };
    (status.code(), reader.join().unwrap())
}

fn assert_refused(program: &Path, config: &Path, code: &str) -> String {
    let (status, stderr) = refused(program, config);
    let line = format!("{PREFIX}refused {code}: ");
    assert!(
        stderr.lines().any(|candidate| candidate.starts_with(&line)),
        "expected a line starting {line:?}; stderr: {stderr:?}"
    );
    assert_eq!(status, Some(1), "{stderr:?}");
    stderr
}

/// One raw HTTP/1.1 exchange with the gateway; the whole answer, as received.
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

fn get(address: SocketAddr, path: &str) -> Vec<u8> {
    exchange(
        address,
        &format!(
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ),
    )
}

fn post(address: SocketAddr, path: &str, body: &str) -> String {
    let answer = exchange(
        address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ),
    );
    String::from_utf8_lossy(&answer).into_owned()
}

// --- A plain `http` fixture endpoint --------------------------------------------------------------

/// One request a fixture endpoint received.
#[derive(Debug, Clone)]
struct Received {
    head: String,
    body: String,
    raw: Vec<u8>,
}

impl Received {
    fn request_line(&self) -> &str {
        self.head.split("\r\n").next().unwrap_or_default()
    }

    /// Every header name, lowercased, in order.
    fn names(&self) -> Vec<String> {
        self.head
            .split("\r\n")
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.trim().to_ascii_lowercase())
            .collect()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.head.split("\r\n").skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// A loopback `http` server answering every request with [`ENDPOINT_ANSWER`] and recording it.
struct Endpoint {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Received>>>,
}

impl Endpoint {
    fn start() -> Self {
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
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ENDPOINT_ANSWER}",
                    ENDPOINT_ANSWER.len()
                );
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
    Some(Received {
        head,
        body: String::from_utf8_lossy(&raw[end + 4..]).into_owned(),
        raw,
    })
}

// --- Criterion 1: the document starts, and the inventory names kinds, never URLs or bytes -------

/// Criterion 1: a document declaring hosted endpoints starts. `GET /v1/routes` lists each hosted
/// model's targets with their `auth_kind` and `billing_kind`, and no byte the process writes, to
/// any client or to standard error, holds a base URL or a byte of a credential file.
#[test]
fn hosted_the_binary_starts_and_lists_each_target_with_its_kinds_and_no_url_or_credential() {
    let fixture = Fixture::new("hosted-inventory");
    let credential = fixture.endpoint_credential();
    let endpoint = Endpoint::start();
    let base_url = endpoint.base_url();
    let other = "https://api.example.test:8443";
    let tables = format!(
        "{}{}{}",
        hosted_table("remote", &base_url, &bearer_keys(&credential)),
        hosted_table(
            "vendor",
            other,
            &format!(
                "{}billing_kind = \"subscription\"\n",
                api_key_keys(&credential, "x-api-key")
            )
        ),
        hosted_table("open", &base_url, "auth_kind = \"anonymous\"\n"),
    );
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ));
    let server = Server::start(&shipped(), &config, None);
    let mut written = Vec::new();
    for path in [
        "/v1/routes",
        "/v1/routes/remote",
        "/v1/routes/vendor",
        "/v1/routes/open",
        "/v1/models",
        "/",
        "/metrics",
    ] {
        written.extend(get(server.address, path));
    }
    let routes = String::from_utf8_lossy(&get(server.address, "/v1/routes")).into_owned();
    written.extend(routes.as_bytes());
    // A relayed request too: the answer is the endpoint's, never its URL or the credential.
    written.extend(
        post(
            server.address,
            "/v1/chat/completions",
            r#"{"model":"remote","messages":[]}"#,
        )
        .as_bytes(),
    );
    for line in server.stop() {
        written.extend(line.as_bytes());
    }

    let body = routes.split("\r\n\r\n").nth(1).unwrap_or_default();
    for (target, auth, billing) in [
        ("remote.chat", "bearer", "metered"),
        ("remote.messages", "bearer", "metered"),
        ("vendor.chat", "api-key", "subscription"),
        ("open.chat", "anonymous", "metered"),
    ] {
        let at = body
            .find(&format!("\"target_id\":\"{target}\""))
            .unwrap_or_else(|| panic!("{target} is not listed: {body}"));
        let entry = &body[at..body[at..].find('}').map_or(body.len(), |end| at + end)];
        assert!(
            entry.contains(&format!("\"auth_kind\":\"{auth}\"")),
            "{target}: {entry}"
        );
        assert!(
            entry.contains(&format!("\"billing_kind\":\"{billing}\"")),
            "{target}: {entry}"
        );
    }
    assert!(routes.starts_with("HTTP/1.1 200 "), "{routes}");
    for withheld in [
        base_url.as_str(),
        &endpoint.address.to_string(),
        other,
        "api.example.test",
        ENDPOINT_CREDENTIAL,
        credential.to_str().unwrap(),
    ] {
        assert!(
            !contains(&written, withheld.as_bytes()),
            "{withheld:?} appears in what the process wrote: {:?}",
            String::from_utf8_lossy(&written)
        );
    }
}

// --- Criteria 2 and 3: each auth kind, on the same path, never the owner's credential ----------

/// The document of three hosted models: `remote` (`bearer`, upstream `vendor/remote-1`) at
/// `bearer`, `keyed` (`api-key` in `X-Api-Key`) at `api_key`, and `open` (`anonymous`) at
/// `anonymous`.
fn relay_config(
    fixture: &Fixture,
    bearer: &Endpoint,
    api_key: &Endpoint,
    anonymous: &Endpoint,
) -> PathBuf {
    let credential = fixture.endpoint_credential();
    let tables = format!(
        "{}{}{}",
        hosted_table(
            "remote",
            &bearer.base_url(),
            &format!(
                "{}upstream_model = \"vendor/remote-1\"\n",
                bearer_keys(&credential)
            )
        ),
        hosted_table(
            "keyed",
            &format!("{}/", api_key.base_url()),
            &api_key_keys(&credential, "X-Api-Key")
        ),
        hosted_table("open", &anonymous.base_url(), "auth_kind = \"anonymous\"\n"),
    );
    fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ))
}

/// Criteria 2 and 3: a relayed request for a hosted model reaches its endpoint at the declared
/// base URL on the same path, with its `model` rewritten to the upstream name. A `bearer`
/// endpoint receives `authorization: Bearer <credential>`; an `api-key` endpoint the credential
/// as its `credential_header`'s value and no `authorization`; an `anonymous` endpoint neither.
/// No byte any endpoint receives holds the owner's credential.
#[test]
fn hosted_the_binary_relays_each_auth_kind_to_its_endpoint_on_the_same_path() {
    let fixture = Fixture::new("hosted-relay");
    let (bearer, api_key, anonymous) = (Endpoint::start(), Endpoint::start(), Endpoint::start());
    let config = relay_config(&fixture, &bearer, &api_key, &anonymous);
    let server = Server::start(&shipped(), &config, None);
    let mut answers = Vec::new();
    for (alias, path) in [
        ("remote", "/v1/chat/completions"),
        ("remote", "/v1/messages"),
        ("keyed", "/v1/chat/completions"),
        ("keyed", "/v1/messages"),
        ("open", "/v1/chat/completions"),
    ] {
        answers.push(post(
            server.address,
            path,
            &format!(r#"{{"model":"{alias}","messages":[]}}"#),
        ));
    }
    let stderr = server.stop();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        assert!(answer.contains(ENDPOINT_ANSWER), "{answer}");
    }

    let remote = bearer.seen();
    assert_eq!(
        remote
            .iter()
            .map(Received::request_line)
            .collect::<Vec<_>>(),
        [
            "POST /v1/chat/completions HTTP/1.1",
            "POST /v1/messages HTTP/1.1"
        ]
    );
    let keyed = api_key.seen();
    assert_eq!(keyed.len(), 2, "{keyed:?}");
    let open = anonymous.seen();
    assert_eq!(open.len(), 1, "{open:?}");
    for request in &remote {
        assert_eq!(request.body, r#"{"model":"vendor/remote-1","messages":[]}"#);
        assert_eq!(
            request.header("host"),
            Some(bearer.address.to_string().as_str())
        );
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {ENDPOINT_CREDENTIAL}").as_str()),
            "{request:?}"
        );
        assert!(!request.names().contains(&"x-api-key".to_owned()));
    }
    for request in &keyed {
        assert_eq!(request.body, r#"{"model":"keyed","messages":[]}"#);
        assert_eq!(
            request.header("x-api-key"),
            Some(ENDPOINT_CREDENTIAL),
            "{request:?}"
        );
        assert!(
            !request.names().contains(&"authorization".to_owned()),
            "{request:?}"
        );
    }
    assert_eq!(open[0].request_line(), "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        open[0].names(),
        ["host", "content-type", "content-length", "connection"],
        "{:?}",
        open[0]
    );
    for request in remote.iter().chain(&keyed).chain(&open) {
        assert!(
            !contains(&request.raw, OWNER_SECRET.as_bytes()),
            "the owner's credential reached an endpoint: {request:?}"
        );
    }
    for line in &stderr {
        assert!(
            !line.contains(ENDPOINT_CREDENTIAL) && !line.contains(OWNER_SECRET),
            "{line}"
        );
    }
}

/// story:hosted-base-path, criteria 1 and 3: an endpoint whose `base_url` carries the path
/// `/api/v1/` is asked on `/api/v1` joined to each wire path, the output clamp's count
/// included, with `host` the authority alone; one whose `base_url` ends in `/v1` is asked on the
/// wire path itself.
#[test]
fn hosted_a_base_url_path_replaces_the_v1_prefix_and_host_stays_the_authority() {
    let fixture = Fixture::new("hosted-base-path");
    let (routed, versioned) = (Endpoint::start(), Endpoint::start());
    let tables = format!(
        "{}{}",
        hosted_table(
            "routed",
            &format!("{}/api/v1/", routed.base_url()),
            "auth_kind = \"anonymous\"\ncontext_window = 8192\n"
        ),
        hosted_table(
            "versioned",
            &format!("{}/v1", versioned.base_url()),
            "auth_kind = \"anonymous\"\n"
        ),
    );
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ));
    let server = Server::start(&shipped(), &config, None);
    let answers = [
        post(
            server.address,
            "/v1/chat/completions",
            r#"{"model":"routed","messages":[]}"#,
        ),
        post(
            server.address,
            "/v1/messages",
            r#"{"model":"routed","max_tokens":16,"messages":[]}"#,
        ),
        post(
            server.address,
            "/v1/chat/completions",
            r#"{"model":"versioned","messages":[]}"#,
        ),
    ];
    server.stop();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    }
    let seen = routed.seen();
    assert_eq!(
        seen.iter().map(Received::request_line).collect::<Vec<_>>(),
        [
            "POST /api/v1/chat/completions HTTP/1.1",
            "POST /api/v1/messages/count_tokens HTTP/1.1",
            "POST /api/v1/messages HTTP/1.1",
        ]
    );
    for request in &seen {
        assert_eq!(
            request.header("host"),
            Some(routed.address.to_string().as_str()),
            "{request:?}"
        );
    }
    assert_eq!(
        versioned
            .seen()
            .iter()
            .map(Received::request_line)
            .collect::<Vec<_>>(),
        ["POST /v1/chat/completions HTTP/1.1"]
    );
}

/// story:hosted-static-headers, criterion 1: an endpoint's declared headers reach it on every
/// request, lowercase and in name order after `host`, beside its credential, and the client's own
/// `anthropic-version` still does not.
#[test]
fn hosted_static_headers_reach_the_endpoint_beside_its_credential() {
    let fixture = Fixture::new("hosted-static-headers");
    let credential = fixture.endpoint_credential();
    let endpoint = Endpoint::start();
    let tables = format!(
        "{}[hosted.claude.headers]\nanthropic-version = \"2023-06-01\"\nX-Title = \"b10x gateway\"\n",
        hosted_table(
            "claude",
            &endpoint.base_url(),
            &api_key_keys(&credential, "x-api-key")
        ),
    );
    let config = fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &tables,
    ));
    let server = Server::start(&shipped(), &config, None);
    let body = r#"{"model":"claude","messages":[]}"#;
    let answer = String::from_utf8_lossy(&exchange(
        server.address,
        &format!(
            "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nanthropic-version: 2099-01-01\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ),
    ))
    .into_owned();
    let chat = post(server.address, "/v1/chat/completions", body);
    server.stop();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(chat.starts_with("HTTP/1.1 200 "), "{chat}");
    let seen = endpoint.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    for request in &seen {
        assert_eq!(
            request.names(),
            [
                "host",
                "anthropic-version",
                "x-title",
                "x-api-key",
                "content-type",
                "content-length",
                "connection"
            ],
            "{request:?}"
        );
        assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(request.header("x-title"), Some("b10x gateway"));
        assert_eq!(request.header("x-api-key"), Some(ENDPOINT_CREDENTIAL));
    }
}

/// A hosted endpoint that fails is answered `upstream-failed` and is kept: the next request
/// reaches the same endpoint, since nothing replaces it. A Runpod model beside it, whose
/// provider declares no connection, is still answered `target-unavailable` (criterion 7: the
/// Runpod composition is unchanged).
#[test]
fn hosted_a_failed_endpoint_is_upstream_failed_and_kept_beside_an_unchanged_runpod_model() {
    let fixture = Fixture::new("hosted-kept");
    let credential = fixture.endpoint_credential();
    // A listener that closes its first connection unanswered, then serves.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let reached = Arc::new(Mutex::new(0_usize));
    let counting = Arc::clone(&reached);
    thread::spawn(move || {
        for (index, connection) in listener.incoming().enumerate() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            if read_request(&mut connection).is_none() {
                continue;
            }
            *counting.lock().unwrap() += 1;
            if index == 0 {
                continue;
            }
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{ENDPOINT_ANSWER}",
                ENDPOINT_ANSWER.len()
            );
            drop(connection.write_all(answer.as_bytes()));
        }
    });
    let secret = fixture.owner_secret();
    let text = format!(
        "{}\n{}",
        document("127.0.0.1:0", &secret),
        hosted_table(
            "remote",
            &format!("http://{address}"),
            &bearer_keys(&credential)
        )
    );
    let config = fixture.config(&text);
    let server = Server::start(&shipped(), &config, None);
    let body = r#"{"model":"remote","messages":[]}"#;
    let first = post(server.address, "/v1/chat/completions", body);
    let second = post(server.address, "/v1/chat/completions", body);
    let runpod = post(
        server.address,
        "/v1/chat/completions",
        r#"{"model":"small","messages":[]}"#,
    );
    server.stop();
    assert!(first.starts_with("HTTP/1.1 502 "), "{first}");
    assert!(first.contains("upstream-failed"), "{first}");
    assert!(second.starts_with("HTTP/1.1 200 "), "{second}");
    assert!(second.contains(ENDPOINT_ANSWER), "{second}");
    assert_eq!(*reached.lock().unwrap(), 2);
    assert!(runpod.starts_with("HTTP/1.1 503 "), "{runpod}");
    assert!(runpod.contains("target-unavailable"), "{runpod}");
}

// --- Criterion 4: https over the binary's verified TLS ------------------------------------------

/// The document of one `bearer` hosted model `remote` at `base_url`.
fn tls_config(fixture: &Fixture, base_url: &str) -> PathBuf {
    let credential = fixture.endpoint_credential();
    fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &hosted_table("remote", base_url, &bearer_keys(&credential)),
    ))
}

/// Criterion 4, first half: an `https` endpoint is reached over TLS, its certificate verified
/// against the binary's trust roots (here the seam's test root) for the base URL's host, and the
/// request and its credential travel inside the TLS connection.
#[test]
fn hosted_an_https_endpoint_is_reached_over_verified_tls() {
    let fixture = Fixture::new("hosted-tls");
    let root = TestRoot::new("hosted test root");
    let root_file = fixture.write("test-root.der", &root.der, 0o600);
    let pod = TlsPod::start(root.server("127.0.0.1"), None);
    let config = tls_config(&fixture, &format!("https://{}", pod.address));
    let server = Server::start(&seam(), &config, Some(&root_file));
    let answer = post(
        server.address,
        "/v1/chat/completions",
        r#"{"model":"remote","messages":[]}"#,
    );
    server.stop();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains(POD_CHAT_ANSWER), "{answer}");
    let seen = pod.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].handshake, "{seen:?}");
    let request = seen[0].text();
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
        "{request}"
    );
    assert!(
        request.contains(&format!(
            "\r\nauthorization: Bearer {ENDPOINT_CREDENTIAL}\r\n"
        )),
        "{request}"
    );
    assert!(!request.contains(OWNER_SECRET), "{request}");
}

/// Criterion 4, second half: an `https` endpoint whose certificate the gateway does not trust
/// ends the request `upstream-failed`, and no request byte reaches it: the seam with a root that
/// did not issue the certificate, and the shipped binary, which trusts only its compiled-in
/// roots, each abandon the handshake.
#[test]
fn hosted_an_untrusted_https_endpoint_is_upstream_failed_and_reads_no_request_byte() {
    let fixture = Fixture::new("hosted-tls-untrusted");
    let trusted = TestRoot::new("hosted trusted root");
    let stranger = TestRoot::new("hosted untrusted root");
    let root_file = fixture.write("test-root.der", &trusted.der, 0o600);
    for (case, program, certificate) in [
        ("an untrusted root", seam(), stranger.server("127.0.0.1")),
        (
            "another host's certificate",
            seam(),
            trusted.server("127.0.0.2"),
        ),
        ("the shipped roots", shipped(), trusted.server("127.0.0.1")),
    ] {
        let pod = TlsPod::start(certificate, None);
        let config = tls_config(&fixture, &format!("https://{}", pod.address));
        let server = Server::start(&program, &config, Some(&root_file));
        let answer = post(
            server.address,
            "/v1/chat/completions",
            r#"{"model":"remote","messages":[]}"#,
        );
        let stderr = server.stop();
        assert!(answer.starts_with("HTTP/1.1 502 "), "{case}: {answer}");
        assert!(answer.contains("upstream-failed"), "{case}: {answer}");
        let deadline = Instant::now() + WAIT;
        let seen = loop {
            let seen = pod.seen();
            if !seen.is_empty() || Instant::now() > deadline {
                break seen;
            }
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(seen.len(), 1, "{case}: {seen:?}");
        assert!(!seen[0].handshake, "{case}: {seen:?}");
        assert!(seen[0].request.is_empty(), "{case}: {seen:?}");
        for line in &stderr {
            assert!(!line.contains(ENDPOINT_CREDENTIAL), "{case}: {line}");
        }
    }
}

// --- Criterion 5: the credential file follows the trusted-file rules ----------------------------

/// The document of one `bearer` hosted model reading `credential`.
fn credential_config(fixture: &Fixture, credential: &Path) -> PathBuf {
    fixture.config(&hosted_document(
        "127.0.0.1:0",
        &fixture.owner_secret(),
        &hosted_table("remote", "http://127.0.0.1:9", &bearer_keys(credential)),
    ))
}

/// Asserts the refusal, that its message names the declaration and the file, and that no byte
/// of `material` reaches standard error.
fn assert_credential_refused(fixture: &Fixture, credential: &Path, code: &str, material: &str) {
    let stderr = assert_refused(&shipped(), &credential_config(fixture, credential), code);
    assert!(
        stderr.contains(&format!(
            "{code}: hosted.remote.credential_file {}:",
            credential.display()
        )),
        "{stderr:?}"
    );
    if !material.trim().is_empty() {
        assert!(
            !stderr.contains(material.trim()),
            "{code}: the refusal quotes the credential: {stderr:?}"
        );
    }
}

/// Criterion 5: a credential file that breaks a trusted-file rule refuses startup with
/// `endpoint-credential:<rule>`, the rules `owner_secret_file` follows, each against a control
/// that starts.
#[test]
fn hosted_a_credential_file_breaking_a_trusted_file_rule_refuses_startup() {
    let fixture = Fixture::new("hosted-credential-rules");
    let credential = fixture.endpoint_credential();
    // Control: the credential file at 0600 starts.
    Server::start(&shipped(), &credential_config(&fixture, &credential), None).stop();

    assert_credential_refused(
        &fixture,
        &fixture.path("absent-credential"),
        "endpoint-credential:unreadable",
        "",
    );
    let link = fixture.path("credential-link");
    symlink(&credential, &link).unwrap();
    assert_credential_refused(
        &fixture,
        &link,
        "endpoint-credential:symlink",
        ENDPOINT_CREDENTIAL,
    );
    assert_credential_refused(
        &fixture,
        &fixture.dir,
        "endpoint-credential:not-regular",
        "",
    );
    for mode in [0o640, 0o604, 0o644, 0o620, 0o602] {
        let path = fixture.write(
            "endpoint-credential",
            format!("{ENDPOINT_CREDENTIAL}\n").as_bytes(),
            mode,
        );
        assert_credential_refused(
            &fixture,
            &path,
            "endpoint-credential:unsafe-mode",
            ENDPOINT_CREDENTIAL,
        );
    }
    // Over the 4098-byte file bound, and a 4097-byte credential inside it once trimmed.
    for material in ["c".repeat(SECRET_LIMIT + 3), "c".repeat(SECRET_LIMIT + 1)] {
        let path = fixture.write("endpoint-credential", material.as_bytes(), 0o600);
        assert_credential_refused(&fixture, &path, "endpoint-credential:too-large", &material);
    }
    let path = fixture.write("endpoint-credential", b"credential-\xff\xfe\n", 0o600);
    assert_credential_refused(&fixture, &path, "endpoint-credential:not-utf8", "");
    for material in [
        "\n".to_owned(),
        format!("{ENDPOINT_CREDENTIAL} {ENDPOINT_CREDENTIAL}\n"),
        format!("{ENDPOINT_CREDENTIAL}\0"),
        format!("{ENDPOINT_CREDENTIAL}\r\nInjected: header\n"),
    ] {
        let path = fixture.write("endpoint-credential", material.as_bytes(), 0o600);
        assert_credential_refused(
            &fixture,
            &path,
            "endpoint-credential:not-a-token",
            &material,
        );
    }
    // Control: a 4096-byte credential and its CRLF starts.
    let path = fixture.write(
        "endpoint-credential",
        format!("{}\r\n", "c".repeat(SECRET_LIMIT)).as_bytes(),
        0o600,
    );
    Server::start(&shipped(), &credential_config(&fixture, &path), None).stop();
}

/// Criterion 5, the owner rule: inside a user namespace that maps this user to uid 4242, a
/// root-owned file appears owned by the overflow uid, neither the effective user nor root.
#[test]
fn hosted_a_credential_file_owned_by_neither_this_user_nor_root_is_refused() {
    let namespaces = Command::new("unshare")
        .args(["--user", "--map-user=4242", "true"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !namespaces {
        eprintln!("SKIPPED: this host refuses unprivileged user namespaces");
        return;
    }
    let fixture = Fixture::new("hosted-credential-owner");
    let config = credential_config(&fixture, Path::new("/etc/passwd"));
    let output = Command::new("unshare")
        .args(["--user", "--map-user=4242"])
        .arg(shipped())
        .args(["--config", config.to_str().unwrap()])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "{PREFIX}refused endpoint-credential:untrusted-owner: "
        )),
        "{stderr:?}"
    );
    assert_eq!(output.status.code(), Some(1), "{stderr:?}");
    // Control: inside the same namespace, a credential file this user owns passes the rule.
    let own = credential_config(&fixture, &fixture.endpoint_credential());
    let mut child = Command::new("unshare")
        .args(["--user", "--map-user=4242"])
        .arg(shipped())
        .args(["--config", own.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    drop(child.kill());
    drop(child.wait());
    assert!(line.starts_with(LISTENING), "{line:?}");
}

// --- Criterion 6: the document's own rules, through the process ---------------------------------

/// Criterion 6 through the binary: an endpoint whose `auth_kind` is not `anonymous` but which
/// names no credential file, and an `api-key` endpoint naming no `credential_header`, are each
/// refused as `config:value` before anything is read or bound.
#[test]
fn hosted_an_endpoint_without_its_credential_file_or_header_refuses_startup_as_config_value() {
    let fixture = Fixture::new("hosted-config-value");
    let credential = fixture.endpoint_credential();
    for (case, keys) in [
        (
            "bearer without a file",
            "auth_kind = \"bearer\"\n".to_owned(),
        ),
        (
            "api-key without a file",
            "auth_kind = \"api-key\"\ncredential_header = \"x-api-key\"\n".to_owned(),
        ),
        (
            "api-key without a header",
            format!(
                "auth_kind = \"api-key\"\ncredential_file = \"{}\"\n",
                credential.display()
            ),
        ),
    ] {
        let config = fixture.config(&hosted_document(
            "127.0.0.1:0",
            &fixture.owner_secret(),
            &hosted_table("remote", "http://127.0.0.1:9", &keys),
        ));
        let stderr = assert_refused(&shipped(), &config, "config:value");
        assert!(stderr.contains("hosted.remote."), "{case}: {stderr:?}");
    }
}
