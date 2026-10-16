//! Adversary pass 1 on story:routes-document: what a route on a Messages endpoint is sent, and a
//! route name llm admits.
//!
//! The endpoint is a loopback fixture recording every request head it reads; nothing leaves the
//! loopback interface and no credential is real.

mod support;

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
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

/// What the Messages endpoint answers a turn with.
const MESSAGES_ANSWER: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);
/// What it answers a count with.
const COUNT_ANSWER: &str = r#"{"input_tokens":12}"#;

/// One request head the endpoint read.
#[derive(Debug, Clone)]
struct Head(String);

impl Head {
    fn request_line(&self) -> &str {
        self.0.split("\r\n").next().unwrap_or_default()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.0.split("\r\n").skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// A loopback Messages endpoint: `count_tokens` answered with [`COUNT_ANSWER`], anything else
/// with [`MESSAGES_ANSWER`]; every head recorded.
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

/// The head and its `content-length` body read off `connection`; the head is returned.
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

/// The binary on `config`, `HOME` set to `home`: the server once it listens, or every line it
/// wrote to standard error when it exits or stays silent instead.
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

    fn post(&self, path: &str, body: &str) -> String {
        self.exchange(&format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }
}

/// A deployment serving only a routes document whose one endpoint, `anthropic`, is a Messages
/// endpoint on the docs' own shape (`api-key` in `x-api-key`, a `file:` key), below
/// `/anthropic/v1` at `endpoint`, with `routes` as its routes section.
fn routes_deployment(fixture: &Fixture, endpoint: SocketAddr, routes: &str) -> std::path::PathBuf {
    let key = fixture.endpoint_credential();
    let document = format!(
        "format: llm.routes/1\nendpoints:\n  anthropic:\n    base_url: http://{endpoint}/anthropic/v1\n    protocol: messages\n    auth: {{kind: api-key, header: x-api-key, secret: \"file:{}\"}}\nroutes:\n{routes}",
        key.display()
    );
    let file = fixture.write("routes.yaml", document.as_bytes(), 0o600);
    let secret = fixture.owner_secret();
    fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\nroutes = \"{}\"\n",
        secret.display(),
        file.display()
    ))
}

/// Claude Code's own request on a route to a Messages endpoint: GET / hands Claude Code exactly
/// this route (the only one on the messages wire with tool calling parsed), and the request it
/// then sends, with `max_tokens` as Claude Code always sends it, is relayed on the endpoint's own
/// wire. Anthropic's Messages API requires `anthropic-version` on every request, and the
/// translated path sends llm's `2023-06-01`; the byte relay must send it too, or the docs' own
/// endpoint `https://api.anthropic.com/v1` answers every same-wire request with an error.
#[test]
fn adversary_routes_a_messages_route_relayed_on_its_own_wire_is_sent_anthropic_version() {
    let fixture = Fixture::new("adv-routes-anthropic-version-relay");
    let endpoint = Endpoint::start();
    let config = routes_deployment(
        &fixture,
        endpoint.address,
        "  opus: anthropic/claude-opus-4-1\n",
    );
    let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
    let profile = server.exchange(
        "GET / HTTP/1.1\r\nhost: gateway\r\nuser-agent: claude-cli/2.0.0 (external, cli)\r\n\r\n",
    );
    assert!(
        profile.contains("export ANTHROPIC_MODEL='opus'"),
        "GET / does not hand Claude Code the route: {profile}"
    );
    let answer = server.post(
        "/v1/messages",
        r#"{"model":"opus","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let seen = endpoint.seen();
    let turn = seen
        .iter()
        .find(|head| head.request_line() == "POST /anthropic/v1/messages HTTP/1.1")
        .unwrap_or_else(|| panic!("the turn never reached the endpoint: {seen:?}"));
    assert_eq!(turn.header("x-api-key"), Some(ENDPOINT_CREDENTIAL));
    assert_eq!(
        turn.header("anthropic-version"),
        Some("2023-06-01"),
        "the relayed turn carries no anthropic-version: {:?}",
        turn.0.replace(ENDPOINT_CREDENTIAL, "<credential>")
    );
}

/// The output clamp's count for that same request: the route's `context_window` (llm's default
/// 200000 for a plain route) makes the relay ask the endpoint's
/// `/messages/count_tokens` first, which Anthropic's API also refuses without
/// `anthropic-version`, so the clamp never clamps there.
#[test]
fn adversary_routes_the_output_clamps_count_to_a_messages_route_is_sent_anthropic_version() {
    let fixture = Fixture::new("adv-routes-anthropic-version-count");
    let endpoint = Endpoint::start();
    let config = routes_deployment(
        &fixture,
        endpoint.address,
        "  opus: anthropic/claude-opus-4-1\n",
    );
    let server = start(&config, &fixture.dir).unwrap_or_else(|stderr| panic!("{stderr:?}"));
    let answer = server.post(
        "/v1/messages",
        r#"{"model":"opus","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let seen = endpoint.seen();
    let count = seen
        .iter()
        .find(|head| head.request_line() == "POST /anthropic/v1/messages/count_tokens HTTP/1.1")
        .unwrap_or_else(|| panic!("no count reached the endpoint: {seen:?}"));
    assert_eq!(count.header("x-api-key"), Some(ENDPOINT_CREDENTIAL));
    assert_eq!(
        count.header("anthropic-version"),
        Some("2023-06-01"),
        "the clamp's count carries no anthropic-version: {:?}",
        count.0.replace(ENDPOINT_CREDENTIAL, "<credential>")
    );
}

/// A route name llm admits (1..=256 printable ASCII bytes without `/`) may still be longer than
/// the gateway serves: its inventory target id `<route>.<wire>` must fit one 256-byte label.
/// Re-pinned in correction round 1, as decided: the gateway publishes its own bound,
/// `MAX_ROUTE_NAME_BYTES`, serves a name at it, and refuses a longer one `config:value` naming
/// the bound, quoting only the start of the name.
#[test]
fn adversary_routes_a_route_name_of_llms_full_length_is_served() {
    let bound = llm_gateway_cli::MAX_ROUTE_NAME_BYTES;
    let document = |name: &str| {
        format!(
            "format: llm.routes/1\nendpoints:\n  lab: {{base_url: http://127.0.0.1:8000/v1, protocol: messages, auth: {{kind: anonymous}}}}\nroutes:\n  {name}: lab/small\n"
        )
    };
    assert!(
        llm_models::routes::Routes::parse(&document(&"r".repeat(256))).is_ok(),
        "llm refuses the name, so the boundary is llm's"
    );
    let endpoint = Endpoint::start();
    let at_bound = Fixture::new("adv-routes-long-name-at-bound");
    let config = routes_deployment(
        &at_bound,
        endpoint.address,
        &format!("  {}: anthropic/claude-opus-4-1\n", "r".repeat(bound)),
    );
    match start(&config, &at_bound.dir) {
        Ok(server) => drop(server),
        Err(stderr) => panic!("a route name at the gateway's bound refused the start: {stderr:?}"),
    }
    for length in [bound + 1, 256] {
        let fixture = Fixture::new(&format!("adv-routes-long-name-{length}"));
        let name = "r".repeat(length);
        let config = routes_deployment(
            &fixture,
            endpoint.address,
            &format!("  {name}: anthropic/claude-opus-4-1\n"),
        );
        let Err(stderr) = start(&config, &fixture.dir) else {
            panic!("a {length}-byte route name was served past the gateway's bound");
        };
        let line = stderr
            .iter()
            .find(|line| line.starts_with("b10x-llm-gateway: refused config:value: routes.r"))
            .unwrap_or_else(|| panic!("{length}: {stderr:?}"));
        assert!(
            line.contains(&format!("at most {bound} bytes")),
            "{length}: {line}"
        );
        assert!(
            !line.contains(&name),
            "{length}: the whole name is echoed: {line}"
        );
    }
}
