//! story:routes-document: a deployment serving the models of an `llm.routes/1` document named by
//! its top-level `routes` key (`spec/domains/deployment.yaml` `RoutesDeclaration`), observed
//! from outside the process.
//!
//! Every endpoint is a loopback fixture recording each request it reads and answering one fixed
//! event stream: a Messages endpoint below `/anthropic/v1` and a Chat Completions endpoint below
//! `/api/v1`, beside a plain `[hosted.*]` endpoint. Nothing leaves the loopback interface and no
//! credential is real.

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
use support::{ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET, bearer_keys, hosted_table};

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const EXIT: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
/// The Chat Completions endpoint's bearer, distinct from the Messages endpoint's key.
const ROUTER_CREDENTIAL: &str = "routes-router-test-credential-not-a-secret";
/// What the plain `[hosted.*]` endpoint answers.
const HOSTED_ANSWER: &str = r#"{"id":"hosted-1","object":"chat.completion"}"#;

/// A messages upstream answering `Hello` with 12 + 2 tokens.
const MESSAGES_ANSWER: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// A chat upstream answering `Hello` with 12 + 2 tokens.
const CHAT_ANSWER: &str = concat!(
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2,\"total_tokens\":14}}\n\n",
    "data: [DONE]\n\n",
);

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// --- A loopback endpoint ----------------------------------------------------------------------

/// One request an endpoint read: its head and its `content-length` body.
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

    fn header(&self, name: &str) -> Option<&str> {
        self.head.split("\r\n").skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }
}

/// A loopback `http` server answering every request with `answer` under `content_type`, and
/// recording each request.
struct Endpoint {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Received>>>,
}

impl Endpoint {
    fn start(content_type: &'static str, answer: &'static str) -> Self {
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
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                drop(connection.write_all(head.as_bytes()));
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self { address, seen }
    }

    fn seen(&self) -> Vec<Received> {
        self.seen.lock().unwrap().clone()
    }
}

fn read_request(connection: &mut TcpStream) -> Option<Received> {
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
    Some(Received {
        head,
        body: String::from_utf8_lossy(&raw[end + 4..]).into_owned(),
        raw,
    })
}

// --- The shipped binary -----------------------------------------------------------------------

fn shipped() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
}

/// The binary run on `config` with `HOME` set to `home`, so a `file:~/` secret resolves there.
fn command(config: &Path, home: &Path) -> Command {
    let mut command = Command::new(shipped());
    command
        .args(["--config", config.to_str().unwrap()])
        .env("HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

/// A served process, read from its standard error line by line; every line is kept.
struct Server {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
    written: Vec<String>,
}

impl Server {
    fn start(config: &Path, home: &Path) -> Self {
        let mut child = command(config, home).spawn().unwrap();
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
                panic!("the binary never reported {LISTENING:?}; stderr: {written:?}");
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

    fn exchange(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn get(&self, path: &str) -> String {
        self.exchange(&format!(
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ))
    }

    fn post(&self, path: &str, body: &str) -> String {
        self.exchange(&format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }
}

/// Runs the binary on `config` to its exit; its exit code and standard error.
fn refused(config: &Path, home: &Path) -> (Option<i32>, String) {
    let mut child = command(config, home).spawn().unwrap();
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

/// The binary refuses `config` with exit status 1 and one line starting
/// `refused <code>: <message>`; that line.
fn assert_refused(config: &Path, home: &Path, code: &str, message: &str) -> String {
    let (status, stderr) = refused(config, home);
    let start = format!("{PREFIX}refused {code}: {message}");
    let line = stderr
        .lines()
        .find(|candidate| candidate.starts_with(&start))
        .unwrap_or_else(|| panic!("expected a line starting {start:?}; stderr: {stderr:?}"))
        .to_owned();
    assert_eq!(status, Some(1), "{stderr:?}");
    assert!(!stderr.contains(LISTENING), "{stderr:?}");
    line
}

// --- The deployment -----------------------------------------------------------------------------

/// The fixtures one deployment runs against, and the files it reads.
struct Deployment {
    fixture: Fixture,
    messages: Endpoint,
    chat: Endpoint,
    hosted: Endpoint,
    /// The Messages endpoint's key, at an absolute path.
    key: PathBuf,
    /// The Chat Completions endpoint's bearer, at `~/router-key`.
    router_key: PathBuf,
}

impl Deployment {
    fn new(name: &str) -> Self {
        let fixture = Fixture::new(name);
        let key = fixture.endpoint_credential();
        let router_key = fixture.write(
            "router-key",
            format!("{ROUTER_CREDENTIAL}\n").as_bytes(),
            0o600,
        );
        Self {
            messages: Endpoint::start("text/event-stream", MESSAGES_ANSWER),
            chat: Endpoint::start("text/event-stream", CHAT_ANSWER),
            hosted: Endpoint::start("application/json", HOSTED_ANSWER),
            fixture,
            key,
            router_key,
        }
    }

    /// The routes document: `opus` on the Messages endpoint `anthropic` with llm's default
    /// capabilities, and `router` on the Chat Completions endpoint `openrouter`, a model id
    /// holding a `/`, with capabilities of its own (no tools, at most 512 output tokens).
    fn routes(&self) -> String {
        format!(
            "format: llm.routes/1
endpoints:
  anthropic:
    base_url: http://{}/anthropic/v1
    protocol: messages
    auth: {{kind: api-key, header: x-api-key, secret: \"file:{}\"}}
  openrouter:
    base_url: http://{}/api/v1
    protocol: chat-completions
    auth: {{kind: bearer, secret: \"file:~/router-key\"}}
routes:
  opus: anthropic/claude-opus-4-1
  router:
    model: openrouter/vendor/model-x
    capabilities: {{tools: false, tool_choice: false, temperature: true, top_p: true, reasoning_efforts: [], context_window: 65536, max_output_tokens: 512}}
",
            self.messages.address,
            self.key.display(),
            self.chat.address,
        )
    }

    /// `routes` written at `routes.yaml` with `mode`.
    fn routes_file(&self, routes: &str, mode: u32) -> PathBuf {
        self.fixture.write("routes.yaml", routes.as_bytes(), mode)
    }

    /// A deployment document naming `routes_file` and `tables` after it.
    fn config_with(&self, routes_file: &Path, tables: &str) -> PathBuf {
        let secret = self.fixture.owner_secret();
        self.fixture.config(&format!(
            "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\nroutes = \"{}\"\n\n{tables}",
            secret.display(),
            routes_file.display()
        ))
    }

    /// The deployment serving [`Self::routes`] and the hosted model `plain`.
    fn config(&self) -> PathBuf {
        let routes = self.routes_file(&self.routes(), 0o600);
        let plain = hosted_table(
            "plain",
            &format!("http://{}", self.hosted.address),
            &bearer_keys(&self.key),
        );
        self.config_with(&routes, &plain)
    }

    fn home(&self) -> &Path {
        &self.fixture.dir
    }
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// The answer's body with its chunked transfer coding removed, when it has one: the bytes the
/// endpoint sent, however the gateway framed them.
fn content(answer: &str) -> String {
    let (head, mut rest) = answer.split_once("\r\n\r\n").unwrap_or((answer, ""));
    if !head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        return rest.to_owned();
    }
    let mut decoded = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        if size == 0 {
            break;
        }
        decoded.push_str(&after[..size]);
        rest = &after[size + 2..];
    }
    decoded
}

/// The object of `body` that starts at `"<key>":"<value>"`, up to its first `}`.
fn entry<'a>(body: &'a str, key: &str, value: &str) -> &'a str {
    let at = body
        .find(&format!("\"{key}\":\"{value}\""))
        .unwrap_or_else(|| panic!("{key} {value} is not listed: {body}"));
    &body[at..body[at..].find('}').map_or(body.len(), |end| at + end)]
}

// --- Acceptance 1 and 5: both routes served, beside a hosted model ---------------------------

/// No request any endpoint received holds the owner's credential, and no line the process wrote
/// holds an endpoint's.
fn assert_no_credential_leaked(requests: &[Received], stderr: &[String]) {
    for request in requests {
        assert!(
            !contains(&request.raw, OWNER_SECRET.as_bytes()),
            "the owner's credential reached an endpoint: {request:?}"
        );
    }
    for line in stderr {
        assert!(
            !line.contains(ENDPOINT_CREDENTIAL) && !line.contains(ROUTER_CREDENTIAL),
            "{line}"
        );
    }
}

/// Acceptance 1 and 5: a request on a route's own endpoint wire is relayed byte for byte to the
/// endpoint's base path, the model rewritten to the route's model id and the credential sent
/// as the endpoint's `auth` says; a `[hosted.*]` model beside the routes is served unchanged.
#[test]
fn routes_a_request_on_the_endpoints_wire_is_relayed_and_hosted_is_unchanged() {
    let deployment = Deployment::new("routes-relayed");
    let server = Server::start(&deployment.config(), deployment.home());
    let same_messages =
        r#"{"model":"opus","stream":true,"messages":[{"role":"user","content":"Hi"}]}"#;
    let same_chat =
        r#"{"model":"router","stream":true,"messages":[{"role":"user","content":"Hi"}]}"#;
    let relayed_messages = server.post("/v1/messages", same_messages);
    let relayed_chat = server.post("/v1/chat/completions", same_chat);
    let hosted = server.post("/v1/chat/completions", r#"{"model":"plain","messages":[]}"#);
    let stderr = server.stop();

    assert!(
        relayed_messages.starts_with("HTTP/1.1 200 "),
        "{relayed_messages}"
    );
    assert_eq!(content(&relayed_messages), MESSAGES_ANSWER);
    assert!(relayed_chat.starts_with("HTTP/1.1 200 "), "{relayed_chat}");
    assert_eq!(content(&relayed_chat), CHAT_ANSWER);
    let messages = deployment.messages.seen();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0].request_line(),
        "POST /anthropic/v1/messages HTTP/1.1"
    );
    assert_eq!(
        messages[0].body,
        same_messages.replace("\"opus\"", "\"claude-opus-4-1\"")
    );
    assert_eq!(
        messages[0].header("host"),
        Some(deployment.messages.address.to_string().as_str())
    );
    assert_eq!(messages[0].header("x-api-key"), Some(ENDPOINT_CREDENTIAL));
    assert_eq!(messages[0].header("authorization"), None);
    let chat = deployment.chat.seen();
    assert_eq!(chat.len(), 1, "{chat:?}");
    assert_eq!(
        chat[0].request_line(),
        "POST /api/v1/chat/completions HTTP/1.1"
    );
    assert_eq!(
        chat[0].body,
        same_chat.replace("\"router\"", "\"vendor/model-x\"")
    );
    let bearer = format!("Bearer {ROUTER_CREDENTIAL}");
    assert_eq!(chat[0].header("authorization"), Some(bearer.as_str()));

    assert!(hosted.starts_with("HTTP/1.1 200 "), "{hosted}");
    assert_eq!(content(&hosted), HOSTED_ANSWER);
    let plain = deployment.hosted.seen();
    assert_eq!(plain.len(), 1, "{plain:?}");
    assert_eq!(plain[0].body, r#"{"model":"plain","messages":[]}"#);
    let requests: Vec<Received> = messages.into_iter().chain(chat).chain(plain).collect();
    assert_no_credential_leaked(&requests, &stderr);
}

/// Acceptance 1: a request on another wire is translated onto the route's endpoint wire, at the
/// same base path, with the endpoint's credential, and answered on the client's own wire.
#[test]
fn routes_a_request_on_another_wire_is_translated_to_the_endpoints_wire() {
    let deployment = Deployment::new("routes-translated");
    let server = Server::start(&deployment.config(), deployment.home());
    let translated_responses = server.post(
        "/v1/responses",
        r#"{"model":"opus","stream":true,"store":false,"include":["reasoning.encrypted_content"],"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}]}"#,
    );
    let translated_messages = server.post(
        "/v1/messages",
        r#"{"model":"router","max_tokens":64,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    let stderr = server.stop();

    assert!(
        translated_responses.starts_with("HTTP/1.1 200 "),
        "{translated_responses}"
    );
    assert!(
        translated_responses.contains("response.completed"),
        "{translated_responses}"
    );
    assert!(
        translated_responses.contains("Hello"),
        "{translated_responses}"
    );
    let messages = deployment.messages.seen();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert_eq!(
        messages[0].request_line(),
        "POST /anthropic/v1/messages HTTP/1.1"
    );
    assert_eq!(messages[0].header("x-api-key"), Some(ENDPOINT_CREDENTIAL));
    assert_eq!(messages[0].header("authorization"), None);
    assert_eq!(messages[0].header("anthropic-version"), Some("2023-06-01"));
    assert!(
        messages[0].body.contains("\"model\":\"claude-opus-4-1\""),
        "{:?}",
        messages[0]
    );

    assert!(
        translated_messages.starts_with("HTTP/1.1 200 "),
        "{translated_messages}"
    );
    assert!(
        translated_messages.contains("\"content\":[{\"text\":\"Hello\",\"type\":\"text\"}]"),
        "{translated_messages}"
    );
    let chat = deployment.chat.seen();
    assert_eq!(chat.len(), 1, "{chat:?}");
    assert_eq!(
        chat[0].request_line(),
        "POST /api/v1/chat/completions HTTP/1.1"
    );
    let bearer = format!("Bearer {ROUTER_CREDENTIAL}");
    assert_eq!(chat[0].header("authorization"), Some(bearer.as_str()));
    assert!(
        chat[0].body.contains("\"model\":\"vendor/model-x\""),
        "{:?}",
        chat[0]
    );
    let requests: Vec<Received> = messages.into_iter().chain(chat).collect();
    assert_no_credential_leaked(&requests, &stderr);
}

/// Rule 5: the route's declared capabilities bound its translated requests and decide its tool
/// calling: `router` takes at most 512 output tokens and no tools, so a translated request for
/// 513 and a same-wire request offering a tool are refused before the endpoint is asked.
#[test]
fn routes_a_routes_declared_capabilities_bound_its_requests() {
    let deployment = Deployment::new("routes-capabilities");
    let server = Server::start(&deployment.config(), deployment.home());
    let over = server.post(
        "/v1/messages",
        r#"{"model":"router","max_tokens":513,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    let tools = server.post(
        "/v1/chat/completions",
        r#"{"model":"router","messages":[{"role":"user","content":"Hi"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]}"#,
    );
    let within = server.post(
        "/v1/messages",
        r#"{"model":"router","max_tokens":512,"messages":[{"role":"user","content":"Hi"}]}"#,
    );
    server.stop();
    assert!(over.starts_with("HTTP/1.1 400 "), "{over}");
    assert!(over.contains("translation-unsupported"), "{over}");
    assert!(tools.starts_with("HTTP/1.1 400 "), "{tools}");
    assert!(tools.contains("tools-not-served"), "{tools}");
    assert!(within.starts_with("HTTP/1.1 200 "), "{within}");
    assert_eq!(deployment.chat.seen().len(), 1);
}

// --- Acceptance 2 and 5: what the listing and the inventory say -------------------------------

/// Acceptance 2 and 5: `GET /v1/models` lists each route on all three wires with its endpoint's
/// wire upstream; `GET /v1/routes` lists one route per route name whose target names the
/// endpoint's protocol, provider `routes`, the endpoint's name and the route's model id, its
/// `auth_kind`, its billing from llm's expansion and the declared limits. No byte the process
/// writes holds a base URL, an endpoint's address, a credential file or a credential.
#[test]
fn routes_the_listing_and_the_inventory_name_each_route_and_no_url_or_credential() {
    let deployment = Deployment::new("routes-inventory");
    let server = Server::start(&deployment.config(), deployment.home());
    let models = server.get("/v1/models");
    let routes = server.get("/v1/routes");
    let mut written = Vec::new();
    for path in ["/v1/routes/opus", "/v1/routes/router", "/", "/metrics"] {
        written.extend(server.get(path).into_bytes());
    }
    written.extend(models.as_bytes());
    written.extend(routes.as_bytes());
    for line in server.stop() {
        written.extend(line.into_bytes());
    }

    assert!(models.starts_with("HTTP/1.1 200 "), "{models}");
    let listing = body(&models);
    for (route, upstream) in [("opus", "messages"), ("router", "chat"), ("plain", "chat")] {
        let listed = entry(listing, "id", route);
        assert!(
            listed.contains("\"wires\":[\"chat\",\"responses\",\"messages\"]"),
            "{listed}"
        );
        let upstream_wires = if route == "plain" {
            "\"upstream_wires\":[\"chat\",\"messages\"]".to_owned()
        } else {
            format!("\"upstream_wires\":[\"{upstream}\"]")
        };
        assert!(listed.contains(&upstream_wires), "{listed}");
    }

    assert!(routes.starts_with("HTTP/1.1 200 "), "{routes}");
    let inventory = body(&routes);
    let opus_target = entry(inventory, "target_id", "opus.messages");
    for field in [
        "\"position\":0",
        "\"protocol\":\"messages\"",
        "\"provider\":\"routes\"",
        "\"account\":\"routes\"",
        "\"endpoint\":\"anthropic\"",
        "\"model\":\"claude-opus-4-1\"",
        "\"auth_kind\":\"api-key\"",
        "\"billing_kind\":\"metered\"",
        "\"context_window\":200000",
        "\"max_output_tokens\":16384",
    ] {
        assert!(opus_target.contains(field), "{field}: {opus_target}");
    }
    let router_target = entry(inventory, "target_id", "router.chat");
    for field in [
        "\"protocol\":\"chat\"",
        "\"provider\":\"routes\"",
        "\"endpoint\":\"openrouter\"",
        "\"model\":\"vendor/model-x\"",
        "\"auth_kind\":\"bearer\"",
        "\"billing_kind\":\"metered\"",
        "\"context_window\":65536",
        "\"max_output_tokens\":512",
    ] {
        assert!(router_target.contains(field), "{field}: {router_target}");
    }
    assert_eq!(
        inventory.matches("\"target_id\":\"opus.").count(),
        1,
        "{inventory}"
    );
    assert!(
        entry(inventory, "target_id", "plain.chat").contains("\"provider\":\"hosted\""),
        "{inventory}"
    );
    for withheld in [
        deployment.messages.address.to_string(),
        deployment.chat.address.to_string(),
        "/anthropic/v1".to_owned(),
        "/api/v1".to_owned(),
        deployment.key.display().to_string(),
        deployment.router_key.display().to_string(),
        "~/router-key".to_owned(),
        ENDPOINT_CREDENTIAL.to_owned(),
        ROUTER_CREDENTIAL.to_owned(),
    ] {
        assert!(
            !contains(&written, withheld.as_bytes()),
            "{withheld:?} appears in what the process wrote: {}",
            String::from_utf8_lossy(&written)
        );
    }
}

/// A deployment whose only models are its routes is admitted and served: `parse` admits a
/// document with `routes` and no `[models.*]` or `[hosted.*]`.
#[test]
fn routes_a_document_with_only_routes_is_admitted_and_served() {
    let text = "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\nroutes = \"routes.yaml\"\n";
    let parsed = llm_gateway_cli::parse(text).unwrap();
    assert!(parsed.models.is_empty() && parsed.hosted.is_empty());
    let deployment = Deployment::new("routes-only");
    let routes = deployment.routes_file(&deployment.routes(), 0o600);
    let config = deployment.config_with(&routes, "");
    let server = Server::start(&config, deployment.home());
    let answer = server.post(
        "/v1/chat/completions",
        r#"{"model":"router","messages":[{"role":"user","content":"Hi"}]}"#,
    );
    let missing = server.post(
        "/v1/chat/completions",
        r#"{"model":"anthropic/claude-opus-4-1","messages":[]}"#,
    );
    server.stop();
    assert_eq!(content(&answer), CHAT_ANSWER);
    // A direct `<endpoint>/<model-id>` name is not a route, and is not served.
    assert!(missing.starts_with("HTTP/1.1 404 "), "{missing}");
}

// --- Acceptance 3 and 4, Rules 1 to 4: what refuses the start ---------------------------------

/// Acceptance 3: an `env:` or `keychain:` secret, a pathless `base_url`, a path outside the
/// base-path rule, a credential over `http` to a remote host, and a document llm refuses each
/// start nothing, refused `config:value` naming what broke the rule.
#[test]
fn routes_each_rule_of_the_document_refuses_the_start_as_config_value() {
    let deployment = Deployment::new("routes-refused");
    let routes = deployment.routes();
    let anthropic_url = format!("http://{}/anthropic/v1", deployment.messages.address);
    let anthropic_secret = format!("\"file:{}\"", deployment.key.display());
    for (case, text, message) in [
        (
            "env",
            routes.replace(&anthropic_secret, "\"env:ANTHROPIC_API_KEY\""),
            "routes.anthropic.auth.secret ",
        ),
        (
            "keychain",
            routes.replace("\"file:~/router-key\"", "\"keychain:llm/router\""),
            "routes.openrouter.auth.secret ",
        ),
        (
            "pathless",
            routes.replace(
                &anthropic_url,
                &format!("http://{}", deployment.messages.address),
            ),
            "routes.anthropic.base_url ",
        ),
        (
            "pathless with a slash",
            routes.replace(
                &anthropic_url,
                &format!("http://{}/", deployment.messages.address),
            ),
            "routes.anthropic.base_url ",
        ),
        (
            "outside the base-path rule",
            routes.replace(
                &anthropic_url,
                &format!("http://{}/a%20b/v1", deployment.messages.address),
            ),
            "routes.anthropic.base_url ",
        ),
        (
            "cleartext credential",
            routes.replace(&anthropic_url, "http://api.example.test/v1"),
            "routes.anthropic.base_url ",
        ),
        (
            "llm refuses an unknown field",
            routes.replace("format: llm.routes/1\n", "format: llm.routes/1\nextra: 1\n"),
            "routes ",
        ),
        (
            "llm refuses a relative file path",
            routes.replace("\"file:~/router-key\"", "\"file:router-key\""),
            "routes ",
        ),
        (
            "llm refuses a literal secret",
            routes.replace("\"file:~/router-key\"", "\"sk-literal\""),
            "routes ",
        ),
        ("not YAML", "format: [".to_owned(), "routes "),
    ] {
        let file = deployment.routes_file(&text, 0o600);
        let config = deployment.config_with(&file, "");
        let line = assert_refused(&config, deployment.home(), "config:value", message);
        assert!(!line.contains("ANTHROPIC_API_KEY"), "{case}: {line}");
        assert!(!line.contains("sk-literal"), "{case}: {line}");
    }
}

/// Rules 2 and 3: a route name equal to a `[hosted.*]` or a `[models.*]` alias, and a `file:~/`
/// secret with no `HOME` to resolve it against, each start nothing, refused `config:value`.
#[test]
fn routes_an_alias_collision_or_a_home_secret_without_home_refuses_the_start() {
    let deployment = Deployment::new("routes-collision");
    let file = deployment.routes_file(&deployment.routes(), 0o600);
    let hosted = hosted_table(
        "opus",
        &format!("http://{}", deployment.hosted.address),
        "auth_kind = \"anonymous\"\n",
    );
    let config = deployment.config_with(&file, &hosted);
    assert_refused(
        &config,
        deployment.home(),
        "config:value",
        "routes.opus names an alias",
    );
    let runpod = "[providers.runpod]\nkind = \"runpod-vllm\"\n\n[models.router]\nprovider = \"runpod\"\nwires = [\"chat\"]\ncontext_window = 65536\nhf_model = \"example/small-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n";
    let config = deployment.config_with(&file, runpod);
    assert_refused(
        &config,
        deployment.home(),
        "config:value",
        "routes.router names an alias",
    );

    // A routes document without a route, and no other model to serve.
    let routes = deployment.routes();
    let empty = deployment.routes_file(routes.split("routes:\n").next().unwrap(), 0o600);
    assert_refused(
        &deployment.config_with(&empty, ""),
        deployment.home(),
        "config:value",
        "models at least one model",
    );
    let file = deployment.routes_file(&deployment.routes(), 0o600);

    // A `file:~/` secret with no home to resolve it against.
    let config = deployment.config_with(&file, "");
    let mut child = command(&config, deployment.home())
        .env_remove("HOME")
        .spawn()
        .unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("refused config:value: routes.openrouter.auth.secret "),
        "{stderr}"
    );
}

/// Rule 1 and acceptance 4: the routes document and each `file:` secret are read through the
/// trusted-file reader. A group-writable routes file is refused as the deployment document is
/// (`config:unsafe-mode`, naming `routes`); a group-readable credential file is refused
/// `endpoint-credential:unsafe-mode`, naming the endpoint's `auth.secret`; neither message
/// quotes a credential. A routes file above llm's 2 MiB bound is `config:too-large`.
#[test]
fn routes_the_routes_file_and_each_credential_file_are_read_through_the_trusted_reader() {
    let deployment = Deployment::new("routes-trusted");
    let routes = deployment.routes();
    let writable = deployment.routes_file(&routes, 0o620);
    let line = assert_refused(
        &deployment.config_with(&writable, ""),
        deployment.home(),
        "config:unsafe-mode",
        "routes: ",
    );
    assert!(line.contains("has mode 620"), "{line}");
    assert!(
        !line.contains("routes.yaml"),
        "the refusal quotes the path: {line}"
    );

    let readable = deployment.routes_file(&routes, 0o644);
    let config = deployment.config_with(&readable, "");
    let server = Server::start(&config, deployment.home());
    server.stop();

    let large = deployment.routes_file(
        &format!("{routes}#{}\n", "a".repeat(2 * 1024 * 1024)),
        0o600,
    );
    assert_refused(
        &deployment.config_with(&large, ""),
        deployment.home(),
        "config:too-large",
        "routes: ",
    );

    let file = deployment.routes_file(&routes, 0o600);
    let config = deployment.config_with(&file, "");
    deployment.fixture.write(
        "router-key",
        format!("{ROUTER_CREDENTIAL}\n").as_bytes(),
        0o640,
    );
    let line = assert_refused(
        &config,
        deployment.home(),
        "endpoint-credential:unsafe-mode",
        "routes.openrouter.auth.secret: ",
    );
    assert!(!line.contains(ROUTER_CREDENTIAL), "{line}");
    deployment
        .fixture
        .write("router-key", b"two tokens\n", 0o600);
    let line = assert_refused(
        &config,
        deployment.home(),
        "endpoint-credential:not-a-token",
        "routes.openrouter.auth.secret: ",
    );
    assert!(!line.contains("two tokens"), "{line}");
}
