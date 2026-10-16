//! Adversary pass 2 on story:routes-document: the correction that sends a Messages route
//! `anthropic-version`, and the published route-name bound.
//!
//! The endpoint is a loopback fixture recording every request head it reads; nothing leaves the
//! loopback interface and no credential is real.

mod support;

use std::{
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
use support::{ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

const MESSAGES_ANSWER: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);
const COUNT_ANSWER: &str = r#"{"input_tokens":12}"#;

#[derive(Debug, Clone)]
struct Head(String);

impl Head {
    fn request_line(&self) -> &str {
        self.0.split("\r\n").next().unwrap_or_default()
    }

    fn values(&self, name: &str) -> Vec<&str> {
        self.0
            .split("\r\n")
            .skip(1)
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
            })
            .collect()
    }

    fn redacted(&self) -> String {
        self.0.replace(ENDPOINT_CREDENTIAL, "<credential>")
    }
}

struct Endpoint {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Head>>>,
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
                let Some(head) = read_head(&mut connection) else {
                    continue;
                };
                let (content_type, answer) = if head.request_line().contains("count_tokens") {
                    ("application/json", COUNT_ANSWER)
                } else {
                    ("text/event-stream", MESSAGES_ANSWER)
                };
                recording.lock().unwrap().push(head);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                drop(connection.write_all(reply.as_bytes()));
            }
        });
        Self { address, seen }
    }

    fn seen(&self) -> Vec<Head> {
        self.seen.lock().unwrap().clone()
    }
}

fn read_head(connection: &mut TcpStream) -> Option<Head> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16_384];
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
    Some(Head(head))
}

struct Server {
    child: Child,
    address: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn start(config: &Path, home: &Path) -> Result<Server, Vec<String>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, lines): (_, Receiver<String>) = mpsc::channel();
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
            return Err(written);
        };
        if let Some(address) = line.strip_prefix(LISTENING) {
            let address = address.parse().unwrap();
            return Ok(Server { child, address });
        }
        written.push(line);
    }
}

impl Server {
    fn exchange(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn post_with(&self, path: &str, extra: &str, body: &str) -> String {
        self.exchange(&format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{extra}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn get(&self, path: &str) -> String {
        self.exchange(&format!(
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ))
    }
}

/// A deployment serving only a routes document with one endpoint `lab` at `endpoint` below
/// `/lab/v1`, speaking `protocol`, authenticated by `auth` (an inline flow mapping, where
/// `{key}` stands for the fixture's credential file), and `routes` as its routes section.
fn deployment(
    fixture: &Fixture,
    endpoint: SocketAddr,
    protocol: &str,
    auth: &str,
    routes: &str,
) -> PathBuf {
    let key = fixture.endpoint_credential();
    let auth = auth.replace("{key}", &key.display().to_string());
    let document = format!(
        "format: llm.routes/1\nendpoints:\n  lab:\n    base_url: http://{endpoint}/lab/v1\n    protocol: {protocol}\n    auth: {auth}\nroutes:\n{routes}"
    );
    let file = fixture.write("routes.yaml", document.as_bytes(), 0o600);
    let secret = fixture.owner_secret();
    fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\nroutes = \"{}\"\n",
        secret.display(),
        file.display()
    ))
}

const API_KEY: &str = "{kind: api-key, header: x-api-key, secret: \"file:{key}\"}";
const ANONYMOUS: &str = "{kind: anonymous}";

/// A client's own `anthropic-version`, `anthropic-beta` and `x-api-key` never reach the route's
/// Messages endpoint: the turn and the clamp's count each carry exactly one `anthropic-version`,
/// llm's, and exactly one `x-api-key`, the endpoint's.
#[test]
fn adversary_routes_p2_a_clients_own_anthropic_headers_never_reach_a_messages_route() {
    let fixture = Fixture::new("adv-routes-p2-client-version");
    let endpoint = Endpoint::start();
    let config = deployment(
        &fixture,
        endpoint.address,
        "messages",
        API_KEY,
        "  opus: lab/claude-opus-4-1\n",
    );
    let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
    let answer = server.post_with(
        "/v1/messages",
        "anthropic-version: 2099-12-31\r\nanthropic-beta: client-beta-1\r\nx-api-key: client-key-not-forwarded\r\n",
        r#"{"model":"opus","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let seen = endpoint.seen();
    for line in [
        "POST /lab/v1/messages HTTP/1.1",
        "POST /lab/v1/messages/count_tokens HTTP/1.1",
    ] {
        let head = seen
            .iter()
            .find(|head| head.request_line() == line)
            .unwrap_or_else(|| panic!("{line} never reached the endpoint: {seen:?}"));
        assert_eq!(
            head.values("anthropic-version"),
            vec![llm_messages::ANTHROPIC_VERSION],
            "{line}: {}",
            head.redacted()
        );
        assert_eq!(
            head.values("x-api-key"),
            vec![ENDPOINT_CREDENTIAL],
            "{line}: {}",
            head.redacted()
        );
        assert!(
            head.values("anthropic-beta").is_empty(),
            "{line}: {}",
            head.redacted()
        );
    }
}

/// A chat request to a Messages route is translated by llm's client, which sends its own
/// `anthropic-version`; the route's static header must not add a second one on that path.
#[test]
fn adversary_routes_p2_a_translated_turn_to_a_messages_route_carries_one_anthropic_version() {
    let fixture = Fixture::new("adv-routes-p2-translated-version");
    let endpoint = Endpoint::start();
    let config = deployment(
        &fixture,
        endpoint.address,
        "messages",
        API_KEY,
        "  opus: lab/claude-opus-4-1\n",
    );
    let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
    let answer = server.post_with(
        "/v1/chat/completions",
        "anthropic-version: 2099-12-31\r\n",
        r#"{"model":"opus","stream":true,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    drop(server);
    let seen = endpoint.seen();
    let turn = seen
        .iter()
        .find(|head| head.request_line() == "POST /lab/v1/messages HTTP/1.1")
        .unwrap_or_else(|| panic!("the translated turn never arrived: {seen:?} / {answer}"));
    assert_eq!(
        turn.values("anthropic-version"),
        vec![llm_messages::ANTHROPIC_VERSION],
        "{}",
        turn.redacted()
    );
}

/// A route on a Chat or Responses endpoint is sent no static header, so no `anthropic-version`,
/// on its own wire.
#[test]
fn adversary_routes_p2_a_non_messages_route_is_sent_no_anthropic_version() {
    for (protocol, path, body) in [
        (
            "chat-completions",
            "/v1/chat/completions",
            r#"{"model":"small","messages":[{"role":"user","content":"Hi"}]}"#,
        ),
        (
            "responses",
            "/v1/responses",
            r#"{"model":"small","input":"Hi"}"#,
        ),
    ] {
        let fixture = Fixture::new(&format!("adv-routes-p2-no-version-{protocol}"));
        let endpoint = Endpoint::start();
        let config = deployment(
            &fixture,
            endpoint.address,
            protocol,
            ANONYMOUS,
            "  small: lab/small\n",
        );
        let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
        let answer = server.post_with(path, "anthropic-version: 2099-12-31\r\n", body);
        drop(server);
        assert!(answer.starts_with("HTTP/1.1 200 "), "{protocol}: {answer}");
        let seen = endpoint.seen();
        assert_eq!(seen.len(), 1, "{protocol}: {seen:?}");
        assert!(
            seen[0].values("anthropic-version").is_empty(),
            "{protocol}: {}",
            seen[0].redacted()
        );
    }
}

/// An `api-key` endpoint on `messages` whose `auth.header` is `Anthropic-Version` (any case)
/// cannot also be sent llm's `anthropic-version`: the start is refused `config:value` naming
/// `routes.lab.auth.header`, and nothing is relayed with the key standing in for the version.
#[test]
fn adversary_routes_p2_an_api_key_header_named_anthropic_version_is_refused_at_start() {
    let fixture = Fixture::new("adv-routes-p2-header-collision");
    let endpoint = Endpoint::start();
    let config = deployment(
        &fixture,
        endpoint.address,
        "messages",
        "{kind: api-key, header: Anthropic-Version, secret: \"file:{key}\"}",
        "  opus: lab/claude-opus-4-1\n",
    );
    let Err(stderr) = start(&config, &fixture.dir) else {
        panic!("a Messages route keyed in anthropic-version was served");
    };
    assert!(
        stderr.iter().any(|line| line
            .starts_with("b10x-llm-gateway: refused config:value: routes.lab.auth.header ")),
        "{stderr:?}"
    );
}

/// The reference page's routes rules list every refusal the start can give a routes document.
/// The correction added one (an `api-key` endpoint on `messages` whose `auth.header` is
/// `anthropic-version`), and the page's `auth.header` rule still says only that it is one a
/// hosted model's `credential_header` may be, which `anthropic-version` is.
#[test]
fn adversary_routes_p2_the_reference_page_publishes_the_anthropic_version_header_refusal() {
    let page = include_str!("../../../website/docs/reference/deployment-document.md");
    let rule = page
        .lines()
        .find(|line| line.contains("naming `routes.<endpoint>.auth.header`"))
        .expect("the page has an auth.header rule");
    assert!(
        rule.contains("anthropic-version"),
        "the auth.header rule does not name the refusal the code gives: {rule}"
    );
}

/// At the bound, on the longest wire: a 246-byte route on a Responses endpoint has a target id
/// `<route>.responses` of exactly 256 bytes. It starts, a request on it is relayed, and the
/// owner's inventory lists that target id.
#[test]
fn adversary_routes_p2_a_route_at_the_bound_on_the_longest_wire_is_served_and_listed() {
    let bound = llm_gateway_cli::MAX_ROUTE_NAME_BYTES;
    assert_eq!(bound, 246, "the published bound");
    let name = "r".repeat(bound);
    let fixture = Fixture::new("adv-routes-p2-bound-responses");
    let endpoint = Endpoint::start();
    let config = deployment(
        &fixture,
        endpoint.address,
        "responses",
        ANONYMOUS,
        &format!("  {name}: lab/small\n"),
    );
    let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
    let answer = server.post_with(
        "/v1/responses",
        "",
        &format!(r#"{{"model":"{name}","input":"Hi"}}"#),
    );
    let routes = server.get("/v1/routes");
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let target = format!("\"target_id\":\"{name}.responses\"");
    assert_eq!(target.len() - "\"target_id\":\"\"".len(), 256);
    assert!(routes.contains(&target), "{routes}");
    assert_eq!(endpoint.seen().len(), 1);
}

/// llm refuses a route name that is not printable ASCII, so the gateway's 32-byte quote of a
/// long name always falls on a character boundary: a multi-byte name never reaches it.
#[test]
fn adversary_routes_p2_a_multi_byte_route_name_never_reaches_the_gateways_bound() {
    for name in [
        format!("{}é", "r".repeat(31)),
        format!("{}é", "r".repeat(250)),
        "é".repeat(124),
    ] {
        let document = format!(
            "format: llm.routes/1\nendpoints:\n  lab: {{base_url: http://127.0.0.1:8000/v1, protocol: messages, auth: {{kind: anonymous}}}}\nroutes:\n  \"{name}\": lab/small\n"
        );
        assert!(
            llm_models::routes::Routes::parse(&document).is_err(),
            "llm admits {name:?}"
        );
        let refused = llm_gateway_cli::Routes::parse(&document).unwrap_err();
        assert!(!refused.message().contains(&name), "{refused}");
    }
}
