//! story:gateway-translation, what the conformance program cannot reach: the shipped binary
//! always composes its translator; a translated request reaches a hosted endpoint on its base
//! path with its credential as its kind says and nothing else of the gateway's; a hosted target
//! reports its scheme; and what llm 0.13.0 does with Claude Code's and Codex's turns: carried and
//! relayed where it carries them, refused by name where it does not; and Codex's two-turn
//! exchange (story:cross-wire-reasoning-state): the reasoning state the gateway sealed for a
//! route is bound on that route's next turn, and every other replayed item is refused.
//!
//! Every upstream is a loopback fixture recording what it receives; nothing leaves the machine
//! and no credential is real.

mod support;

use llm_gateway::{
    Label, RefusalCode, RelayModel, RelayTarget, RelayTargets, TargetRefusal, TargetScheme,
    ToolCalling, TranslationRequest, Translator, Wire,
};
use llm_gateway_cli::{
    EndpointCredential, HostedEndpoint, HostedTargets, LlmTranslator, ProxyConnector,
};
use llm_messages::{SYSTEM_MESSAGE_CLOSE, SYSTEM_MESSAGE_OPEN};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use support::{ENDPOINT_CREDENTIAL, Fixture, OWNER_SECRET, hosted_document};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

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

// --- A loopback upstream -----------------------------------------------------------------------

/// Answers each connection with `answer` as an event stream, recording every request it read.
struct Upstream {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Upstream {
    fn start(answers: usize, answer: &'static str) -> Self {
        Self::script(vec![answer; answers])
    }

    /// Answers its first connection with the first of `answers`, the next with the next, and
    /// accepts no more.
    fn script(answers: Vec<&'static str>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let thread = thread::spawn(move || {
            for answer in answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let request = read_request(&mut connection);
                recording.lock().unwrap().push(request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                drop(connection.write_all(head.as_bytes()));
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self {
            address,
            seen,
            thread: Some(thread),
        }
    }

    fn seen(mut self) -> Vec<String> {
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|raw| String::from_utf8_lossy(raw).into_owned())
            .collect()
    }
}

/// One request: its head, then a `content-length` body.
fn read_request(connection: &mut TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16_384];
    let end = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return raw,
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
    raw
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    let head = request.split("\r\n\r\n").next()?;
    head.split("\r\n").skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

// --- The shipped binary ------------------------------------------------------------------------

struct Server {
    child: Child,
    address: SocketAddr,
    /// Every line the binary wrote to standard error before it reported its address.
    startup: Vec<String>,
    /// Every line the binary wrote to standard error after it reported its address.
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(config: &Path) -> Self {
        Self::start_logging(config, "info")
    }

    /// The binary with `RUST_LOG` set to `filter`.
    fn start_logging(config: &Path, filter: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
            .args(["--config", config.to_str().unwrap()])
            .env("RUST_LOG", filter)
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
        let mut startup = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = lines.recv_timeout(left).unwrap_or_else(|_| {
                drop(child.kill());
                panic!("the binary never reported {LISTENING:?}: {startup:#?}");
            });
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
            startup.push(line);
        };
        Self {
            child,
            address,
            startup,
            lines,
        }
    }

    /// Stops the binary and returns every line it wrote to standard error after its address.
    fn log(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        let mut lines = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(WAIT) {
            lines.push(line);
        }
        lines
    }

    fn post(&self, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            stream,
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// A `GET` of `path` without a credential, with `user_agent`; the whole answer.
    fn get(&self, path: &str, user_agent: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nuser-agent: {user_agent}\r\n\r\n"
        )
        .unwrap();
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        String::from_utf8_lossy(&raw).into_owned()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

fn document(fixture: &Fixture, table: &str) -> PathBuf {
    let secret = fixture.owner_secret();
    fixture.config(&hosted_document("127.0.0.1:0", &secret, table))
}

/// The binary composes its translator for every composition: a chat request for a model whose
/// endpoint speaks only messages reaches that endpoint translated, on its base path joined to
/// `/messages`, with its API key in its own header, llm's `anthropic-version`, and neither the
/// endpoint's static headers nor `authorization` nor the owner's credential.
#[test]
fn translation_the_binary_reaches_an_api_key_endpoint_on_its_base_path_with_its_key_only() {
    let fixture = Fixture::new("translation-api-key");
    let credential = fixture.endpoint_credential();
    let upstream = Upstream::start(1, MESSAGES_ANSWER);
    let table = format!(
        "[hosted.local]\nwires = [\"messages\"]\nbase_url = \"http://{}/api/v1\"\nupstream_model = \"served-local\"\nauth_kind = \"api-key\"\ncredential_file = \"{}\"\ncredential_header = \"x-api-key\"\n[hosted.local.headers]\nX-Title = \"b10x gateway\"\n",
        upstream.address,
        credential.display()
    );
    let server = Server::start(&document(&fixture, &table));
    let answer = server.post(
        "/v1/chat/completions",
        "{\"model\":\"local\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}",
    );
    drop(server);
    let seen = upstream.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let request = &seen[0];
    assert!(
        request.starts_with("POST /api/v1/messages HTTP/1.1\r\n"),
        "{request}"
    );
    assert_eq!(
        header(request, "x-api-key"),
        Some(ENDPOINT_CREDENTIAL),
        "{request}"
    );
    assert_eq!(
        header(request, "anthropic-version"),
        Some("2023-06-01"),
        "{request}"
    );
    assert_eq!(header(request, "authorization"), None, "{request}");
    assert_eq!(header(request, "x-title"), None, "{request}");
    assert!(!request.contains(OWNER_SECRET), "{request}");
    assert!(request.contains("\"model\":\"served-local\""), "{request}");
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("content-type: text/event-stream\r\n"),
        "{answer}"
    );
    assert!(answer.contains("\"content\":\"Hello\""), "{answer}");
    assert!(answer.contains("data: [DONE]"), "{answer}");
}

/// A bearer endpoint without a base path is reached on `/v1` joined to the upstream wire's
/// path, with its credential as `authorization: Bearer`, and never the owner's.
#[test]
fn translation_the_binary_reaches_a_bearer_endpoint_on_v1_with_its_bearer_only() {
    let fixture = Fixture::new("translation-bearer");
    let credential = fixture.endpoint_credential();
    let upstream = Upstream::start(1, CHAT_ANSWER);
    let table = format!(
        "[hosted.local]\nwires = [\"chat\"]\nbase_url = \"http://{}\"\nauth_kind = \"bearer\"\ncredential_file = \"{}\"\n",
        upstream.address,
        credential.display()
    );
    let server = Server::start(&document(&fixture, &table));
    let answer = server.post(
        "/v1/messages",
        "{\"model\":\"local\",\"max_tokens\":64,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}",
    );
    drop(server);
    let seen = upstream.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let request = &seen[0];
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
        "{request}"
    );
    let bearer = format!("Bearer {ENDPOINT_CREDENTIAL}");
    assert_eq!(
        header(request, "authorization"),
        Some(bearer.as_str()),
        "{request}"
    );
    assert!(!request.contains(OWNER_SECRET), "{request}");
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("content-type: application/json\r\n"),
        "{answer}"
    );
    assert!(
        answer.contains("\"content\":[{\"text\":\"Hello\",\"type\":\"text\"}]"),
        "{answer}"
    );
    assert!(answer.contains("\"stop_reason\":\"end_turn\""), "{answer}");
}

struct Nothing;

impl RelayTargets for Nothing {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// A hosted target reports the scheme of its base URL, which the translator reaches it over.
#[test]
fn translation_a_hosted_target_reports_its_base_urls_scheme() {
    let mut endpoints = BTreeMap::new();
    for (alias, url) in [
        ("plain", "http://127.0.0.1:8000/v1"),
        ("tls", "https://api.example.test/api/v1"),
    ] {
        endpoints.insert(
            alias.to_owned(),
            HostedEndpoint::new(url, EndpointCredential::Anonymous).unwrap(),
        );
    }
    let targets = HostedTargets::new(
        endpoints,
        Arc::new(ProxyConnector::shipped()),
        Arc::new(Nothing),
    );
    let scheme = |alias: &str| targets.acquire(alias).map(|target| target.scheme()).ok();
    assert_eq!(scheme("plain"), Some(TargetScheme::Http));
    assert_eq!(scheme("tls"), Some(TargetScheme::Https));
}

fn relay_model(alias: &str, wires: Vec<Wire>) -> RelayModel {
    RelayModel::new(
        Label::new(alias).unwrap(),
        Label::new("served").unwrap(),
        wires,
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed)
}

fn prepared(ingress: Wire, upstream: Wire, body: &[u8]) -> Option<RefusalCode> {
    let translator = LlmTranslator::new();
    let model = relay_model("code", vec![upstream]);
    translator
        .prepare(&TranslationRequest {
            ingress,
            upstream,
            model: &model,
            body,
            anthropic_beta: None,
            caller: llm_gateway::Caller::default(),
        })
        .err()
}

/// The body of an authored fixture request: everything after its provenance header and its
/// HTTP head.
fn fixture_body(text: &str) -> String {
    let request = text.lines().skip_while(|line| line.starts_with('#')).fold(
        String::new(),
        |mut request, line| {
            request.push_str(line);
            request.push('\n');
            request
        },
    );
    request.split_once("\n\n").unwrap().1.to_owned()
}

/// Claude Code's first turn (the authored fixture of `crates/llm-gateway`).
const CLAUDE_CODE_FIRST_TURN: &str =
    include_str!("../../llm-gateway/tests/fixtures/claude-code/1-tool-turn.request.txt");
/// Codex 0.160.0's first and second turns, captured and sanitized in llm 0.7.0
/// (`tests/fixtures/codex/README.md`).
const CODEX_FIRST_TURN: &str = include_str!("fixtures/codex/first-turn.json");
const CODEX_SECOND_TURN: &str = include_str!("fixtures/codex/second-turn.json");

/// The JSON body of the one request `seen` holds.
fn upstream_body(seen: &[String]) -> serde_json::Value {
    assert_eq!(seen.len(), 1, "{seen:?}");
    serde_json::from_str(seen[0].split_once("\r\n\r\n").unwrap().1).unwrap()
}

/// Posts `body` on `path` to the shipped binary serving `code` from a hosted endpoint that
/// speaks only `wire`, and returns the gateway's answer and the body the endpoint received.
fn through_the_binary(
    name: &str,
    wire: &str,
    answer: &'static str,
    path: &str,
    body: &str,
) -> (String, serde_json::Value) {
    let fixture = Fixture::new(name);
    let upstream = Upstream::start(1, answer);
    let table = format!(
        "[hosted.code]\nwires = [\"{wire}\"]\nbase_url = \"http://{}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n",
        upstream.address
    );
    let server = Server::start(&document(&fixture, &table));
    let answer = server.post(path, body);
    drop(server);
    // A refused turn never reaches the upstream, whose recorder would wait for it forever.
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    (answer, upstream_body(&upstream.seen()))
}

/// llm 0.13.0's messages decoder's refusal of `body`, read as a client's.
fn messages_refusal(body: &serde_json::Value) -> (llm_core::ErrorCode, String) {
    let origin = llm_core::Provenance {
        protocol: llm_core::Protocol::Messages,
        provider: llm_core::Id::new("client").unwrap(),
        account: llm_core::Id::new("client").unwrap(),
        endpoint: llm_core::Id::new("ingress").unwrap(),
        model: llm_core::Id::new("code").unwrap(),
        binding_revision: llm_core::Id::new("ingress").unwrap(),
    };
    let error = llm_messages::decode_request(body.to_string().as_bytes(), &origin).unwrap_err();
    (error.code, error.message)
}

/// Claude Code's first turn is carried to a chat and a responses upstream: `thinking`
/// adaptive, a `system` of two text blocks, `cache_control` on a block and effort `max`, which
/// reaches either wire as `xhigh`. Through the binary, the chat upstream is sent the system
/// blocks as one system message, the effort as `xhigh`, the tools, and no `cache_control`,
/// `thinking` or `output_config`; the client is answered on its own wire.
#[test]
fn translation_claude_codes_first_turn_is_carried_to_chat_and_responses() {
    let body = fixture_body(CLAUDE_CODE_FIRST_TURN);
    for upstream in [Wire::Chat, Wire::Responses] {
        assert_eq!(
            prepared(Wire::Messages, upstream, body.as_bytes()),
            None,
            "Claude Code's first turn to {upstream:?}"
        );
    }
    let (answer, sent) = through_the_binary(
        "translation-claude-code",
        "chat",
        CHAT_ANSWER,
        "/v1/messages",
        &body,
    );
    let fixture: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(sent["model"], "served", "{sent}");
    assert_eq!(sent["reasoning_effort"], "xhigh", "{sent}");
    assert_eq!(
        sent["max_completion_tokens"], fixture["max_tokens"],
        "{sent}"
    );
    assert_eq!(sent["messages"][0]["role"], "system", "{sent}");
    let system = sent["messages"][0]["content"].as_str().unwrap();
    for block in fixture["system"].as_array().unwrap() {
        assert!(system.contains(block["text"].as_str().unwrap()), "{system}");
    }
    assert_eq!(
        sent["tools"].as_array().unwrap().len(),
        fixture["tools"].as_array().unwrap().len(),
        "{sent}"
    );
    let text = sent.to_string();
    for absent in ["cache_control", "thinking", "output_config", "max_tokens\""] {
        assert!(
            !text.contains(absent),
            "{absent} reached a chat upstream: {text}"
        );
    }
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("event: message_stop"), "{answer}");
    assert!(answer.contains("\"text\":\"Hello\""), "{answer}");
}

/// Codex's first turn is carried to a chat upstream: `instructions` and the developer message
/// that opens `input` become the system message, `parallel_tool_calls`, `prompt_cache_key` and
/// the effort reach it, and the item ids, `client_metadata` and the reasoning summary, which no
/// chat request carries, do not. The client is answered on the responses wire.
#[test]
fn translation_codexs_first_turn_is_carried_to_chat() {
    let codex: serde_json::Value = serde_json::from_str(CODEX_FIRST_TURN).unwrap();
    let (answer, sent) = through_the_binary(
        "translation-codex-chat",
        "chat",
        CHAT_ANSWER,
        "/v1/responses",
        CODEX_FIRST_TURN,
    );
    assert_eq!(sent["model"], "served", "{sent}");
    assert_eq!(
        sent["parallel_tool_calls"], codex["parallel_tool_calls"],
        "{sent}"
    );
    assert_eq!(
        sent["prompt_cache_key"], codex["prompt_cache_key"],
        "{sent}"
    );
    assert_eq!(
        sent["reasoning_effort"], codex["reasoning"]["effort"],
        "{sent}"
    );
    assert_eq!(sent["messages"][0]["role"], "system", "{sent}");
    let system = sent["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.starts_with(codex["instructions"].as_str().unwrap()),
        "{system}"
    );
    for part in codex["input"][0]["content"].as_array().unwrap() {
        assert!(system.contains(part["text"].as_str().unwrap()), "{system}");
    }
    assert_eq!(
        sent["tools"].as_array().unwrap().len(),
        codex["tools"].as_array().unwrap().len(),
        "{sent}"
    );
    let text = sent.to_string();
    for absent in ["msg_00000000", "client_metadata", "x-codex", "summary"] {
        assert!(
            !text.contains(absent),
            "{absent} reached a chat upstream: {text}"
        );
    }
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("event: response.completed"), "{answer}");
}

/// Codex's first turn is carried to a messages upstream: the instructions are its `system`,
/// `parallel_tool_calls` false is `tool_choice.disable_parallel_tool_use`, the effort is
/// `output_config.effort`, and a request naming no output limit is given the default one.
#[test]
fn translation_codexs_first_turn_is_carried_to_messages() {
    let codex: serde_json::Value = serde_json::from_str(CODEX_FIRST_TURN).unwrap();
    let (answer, sent) = through_the_binary(
        "translation-codex-messages",
        "messages",
        MESSAGES_ANSWER,
        "/v1/responses",
        CODEX_FIRST_TURN,
    );
    assert_eq!(sent["model"], "served", "{sent}");
    assert_eq!(sent["max_tokens"], 16_384, "{sent}");
    assert_eq!(
        sent["output_config"]["effort"], codex["reasoning"]["effort"],
        "{sent}"
    );
    assert_eq!(
        sent["tool_choice"]["disable_parallel_tool_use"],
        serde_json::json!(codex["parallel_tool_calls"] == false),
        "{sent}"
    );
    assert!(
        sent["system"]
            .to_string()
            .contains(codex["instructions"].as_str().unwrap()),
        "{sent}"
    );
    let text = sent.to_string();
    for absent in [
        "msg_00000000",
        "client_metadata",
        "prompt_cache_key",
        "summary",
    ] {
        assert!(
            !text.contains(absent),
            "{absent} reached a messages upstream: {text}"
        );
    }
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("event: response.completed"), "{answer}");
}

// --- Which clients a translated model is profiled for (story:client-profiles-translated) -------

/// The translator admits Codex and Claude Code exactly where llm carries their recorded first
/// turns, the ones the cases above send: its answer and `prepare`'s agree for every upstream
/// wire either is translated to.
#[test]
fn translation_codex_and_claude_code_are_admitted_where_llm_carries_their_first_turns() {
    let translator = LlmTranslator::new();
    let claude_code = fixture_body(CLAUDE_CODE_FIRST_TURN);
    for (client, body, upstreams) in [
        (
            Wire::Responses,
            CODEX_FIRST_TURN,
            [Wire::Chat, Wire::Messages],
        ),
        (
            Wire::Messages,
            claude_code.as_str(),
            [Wire::Chat, Wire::Responses],
        ),
    ] {
        for upstream in upstreams {
            let model = relay_model("code", vec![upstream]);
            assert_eq!(
                prepared(client, upstream, body.as_bytes()),
                None,
                "{client:?} to {upstream:?}"
            );
            assert!(
                translator.admits(client, &model, upstream),
                "{client:?} to {upstream:?}"
            );
        }
    }
}

/// Where llm refuses a client's recorded first turn, the client is not admitted. Claude Code's
/// turn asks for effort `max`, which reaches a chat upstream as `xhigh`, and 32000 output
/// tokens: a route under llm's default capabilities (efforts `low` to `high`, 16384 output
/// tokens) refuses it, and so does one lacking either `xhigh` or the 32000, while one with both
/// admits it. Codex's turn, at effort `high` with no output limit, is admitted under the
/// defaults. Neither client's tools reach a model whose tool calling is absent. A Loom run, of
/// which no turn is recorded, is admitted nowhere.
#[test]
fn translation_a_client_is_not_admitted_where_llm_refuses_its_first_turn() {
    let chat = relay_model("code", vec![Wire::Chat]);
    let routed =
        LlmTranslator::new().with_capabilities("code", llm_models::routes::default_capabilities());
    assert!(!routed.admits(Wire::Messages, &chat, Wire::Chat));
    assert!(routed.admits(Wire::Responses, &chat, Wire::Chat));
    let under = |efforts: &[&str], output: u64| {
        let mut capabilities = llm_models::routes::default_capabilities();
        capabilities.reasoning_efforts = efforts.iter().map(|&effort| effort.to_owned()).collect();
        capabilities.max_output_tokens = output;
        LlmTranslator::new().with_capabilities("code", capabilities)
    };
    let up_to_xhigh = ["low", "medium", "high", "xhigh"];
    for (efforts, output, admitted) in [
        (&up_to_xhigh[..], 16_384, false),
        (&up_to_xhigh[..3], 32_000, false),
        (&up_to_xhigh[..], 32_000, true),
    ] {
        assert_eq!(
            under(efforts, output).admits(Wire::Messages, &chat, Wire::Chat),
            admitted,
            "efforts {efforts:?}, {output} output tokens"
        );
    }
    let translator = LlmTranslator::new();
    let text_only = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    assert!(!translator.admits(Wire::Responses, &text_only, Wire::Chat));
    assert!(!translator.admits(Wire::Messages, &text_only, Wire::Chat));
    for upstream in [Wire::Responses, Wire::Messages] {
        let model = relay_model("code", vec![upstream]);
        assert!(
            !translator.admits(Wire::Chat, &model, upstream),
            "{upstream:?}"
        );
    }
}

/// Through the binary, `GET /` hands Codex and Claude Code the hosted models their first turns
/// translate to beside the ones they reach natively, and a Loom run only the model on its own
/// wire; a model without tool calling no one. Composing asks about every model and writes no
/// `translation refused` line, although the model without tool calling refuses both turns.
#[test]
fn translation_get_root_profiles_codex_and_claude_code_where_their_turns_translate() {
    let fixture = Fixture::new("translation-profiles");
    let hosted = |alias: &str, wire: &str, tools: &str| {
        format!(
            "[hosted.{alias}]\nwires = [\"{wire}\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\n{tools}"
        )
    };
    let parsed = "tool_calling = \"parsed\"\n";
    let table = [
        hosted("chat-up", "chat", parsed),
        hosted("msg-up", "messages", parsed),
        hosted("notools", "chat", ""),
    ]
    .concat();
    let server = Server::start(&document(&fixture, &table));
    let startup = server.startup.clone();
    let codex = server.get("/", "codex-cli/1.0");
    let claude = server.get("/", "claude-cli/2.1.293");
    let loom = server.get("/", "curl/8.0");
    let log = server.log();
    assert!(codex.contains("model = \"chat-up\"\n"), "{codex}");
    assert!(codex.contains("model = \"msg-up\"\n"), "{codex}");
    assert!(
        claude.contains("export ANTHROPIC_MODEL='chat-up'\n"),
        "{claude}"
    );
    assert!(
        claude.contains("export ANTHROPIC_MODEL='msg-up'\n"),
        "{claude}"
    );
    assert!(loom.contains("upstream_name = \"chat-up\"\n"), "{loom}");
    assert!(!loom.contains("msg-up"), "{loom}");
    for answer in [&codex, &claude, &loom] {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        assert!(!answer.contains("notools"), "{answer}");
    }
    for line in startup.iter().chain(&log) {
        assert!(!line.contains("translation refused"), "{line}");
        assert!(!line.contains("dropped a field"), "{line}");
    }
}

/// The user prompt and environment block of Claude Code 2.1.300's headless request, whose
/// `messages` are `[user, system]`, the system entry a string.
const HEADLESS_PROMPT: &str = "prompt-marker-list-the-crates";
const HEADLESS_ENVIRONMENT: &str =
    "<env>\nWorking directory: /workspace\nIs directory a git repo: Yes\nPlatform: linux\n</env>";

/// A Messages request for `code` whose `messages` are `messages`.
fn messages_request(messages: &serde_json::Value) -> String {
    serde_json::json!({"model": "code", "max_tokens": 64, "messages": messages}).to_string()
}

/// The text of a chat message, a string or the text of each part, in order.
fn chat_texts(message: &serde_json::Value) -> Vec<String> {
    match &message["content"] {
        serde_json::Value::String(text) => vec![text.clone()],
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Claude Code 2.1.300's headless request, `[user, system]`, reaches a chat upstream with 200:
/// the system entry follows the user text, as user text in llm's mark, and no system message
/// carries it.
#[test]
fn translation_a_later_system_entry_reaches_a_chat_upstream_as_marked_user_text() {
    let body = messages_request(&serde_json::json!([
        {"role": "user", "content": HEADLESS_PROMPT},
        {"role": "system", "content": HEADLESS_ENVIRONMENT},
    ]));
    let (answer, sent) = through_the_binary(
        "translation-later-system",
        "chat",
        CHAT_ANSWER,
        "/v1/messages",
        &body,
    );
    assert!(answer.contains("\"text\":\"Hello\""), "{answer}");
    let messages = sent["messages"].as_array().unwrap();
    assert!(
        messages.iter().all(|message| message["role"] == "user"),
        "{sent}"
    );
    let texts: Vec<String> = messages.iter().flat_map(chat_texts).collect();
    assert_eq!(
        texts,
        [
            HEADLESS_PROMPT.to_owned(),
            format!("{SYSTEM_MESSAGE_OPEN}{HEADLESS_ENVIRONMENT}{SYSTEM_MESSAGE_CLOSE}"),
        ],
        "{sent}"
    );
}

/// A leading system entry inside `messages` joins the system prompt: a chat upstream is sent it
/// as the system message, and no user text carries the mark.
#[test]
fn translation_a_leading_system_entry_reaches_a_chat_upstream_as_the_system_message() {
    let body = messages_request(&serde_json::json!([
        {"role": "system", "content": HEADLESS_ENVIRONMENT},
        {"role": "user", "content": HEADLESS_PROMPT},
    ]));
    let (answer, sent) = through_the_binary(
        "translation-leading-system",
        "chat",
        CHAT_ANSWER,
        "/v1/messages",
        &body,
    );
    assert!(answer.contains("\"text\":\"Hello\""), "{answer}");
    assert_eq!(sent["messages"][0]["role"], "system", "{sent}");
    assert_eq!(
        chat_texts(&sent["messages"][0]).concat(),
        HEADLESS_ENVIRONMENT,
        "{sent}"
    );
    assert_eq!(sent["messages"][1]["role"], "user", "{sent}");
    assert_eq!(
        chat_texts(&sent["messages"][1]),
        [HEADLESS_PROMPT],
        "{sent}"
    );
    assert!(!sent.to_string().contains("<system-message>"), "{sent}");
}

/// The prefix of every reasoning seal the gateway hands a client (docs/gateway.md "Reasoning
/// state on a translated route").
const SEAL_PREFIX: &str = "llmgw-seal-1.";

/// A messages upstream that thinks, signs its thinking and calls Codex's `exec_command`, as
/// Codex's recorded second turn says its first answer did.
const MESSAGES_THINKING_CALL: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Read the file.\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-upstream-1\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_capture_1\",\"name\":\"exec_command\",\"input\":{}}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\": \\\"cat a.txt\\\"}\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":9}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// A chat upstream that shows its reasoning as `reasoning_content` and calls `exec_command`.
const CHAT_REASONING_CALL: &str = concat!(
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Read the file.\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_capture_1\",\"type\":\"function\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\": \\\"cat a.txt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":9,\"total_tokens\":21}}\n\n",
    "data: [DONE]\n\n",
);

/// The payloads of a responses event stream the gateway answered with.
fn events(answer: &str) -> Vec<serde_json::Value> {
    answer
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// The finished reasoning item a responses answer handed the client.
fn reasoning_item(answer: &str) -> serde_json::Value {
    events(answer)
        .into_iter()
        .find(|event| {
            event["type"] == "response.output_item.done" && event["item"]["type"] == "reasoning"
        })
        .map_or_else(
            || panic!("no finished reasoning item reached the client: {answer}"),
            |event| event["item"].clone(),
        )
}

/// Codex's recorded second turn, its reasoning item replayed as `item` says: Codex replays the
/// item its first answer carried with that item's `id`, summary and `encrypted_content`.
fn codex_second_turn(model: &str, item: &serde_json::Value) -> String {
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_SECOND_TURN).unwrap();
    codex["model"] = serde_json::json!(model);
    let replayed = codex["input"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["type"] == "reasoning")
        .unwrap();
    for key in ["id", "summary", "encrypted_content"] {
        replayed[key] = item[key].clone();
    }
    codex.to_string()
}

/// The JSON body of one request an upstream recorded.
fn request_body(request: &str) -> serde_json::Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

/// A hosted model `alias` served by a `wire` endpoint at `upstream`.
fn hosted_model(alias: &str, wire: &str, upstream: &Upstream) -> String {
    format!(
        "[hosted.{alias}]\nwires = [\"{wire}\"]\nbase_url = \"http://{}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n",
        upstream.address
    )
}

/// Codex's recorded two-turn exchange through a responses-to-messages route: the first answer
/// hands Codex a finished reasoning item sealed by the gateway, and the second turn, replaying
/// it, is relayed to the messages upstream with the signed thinking block it minted, byte for
/// byte, before the call it made and the call's result.
#[test]
fn translation_codexs_two_turns_reach_a_messages_upstream_with_the_thinking_it_signed() {
    let fixture = Fixture::new("translation-codex-two-turns-messages");
    let upstream = Upstream::script(vec![MESSAGES_THINKING_CALL, MESSAGES_ANSWER]);
    let server = Server::start(&document(
        &fixture,
        &hosted_model("code", "messages", &upstream),
    ));
    let first = server.post("/v1/responses", CODEX_FIRST_TURN);
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    let item = reasoning_item(&first);
    assert_eq!(
        item["summary"],
        serde_json::json!([{"type": "summary_text", "text": "Read the file."}]),
        "{item}"
    );
    let sealed = item["encrypted_content"].as_str().unwrap();
    assert!(sealed.starts_with(SEAL_PREFIX), "{item}");
    assert!(first.contains("\"call_id\":\"call_capture_1\""), "{first}");
    let second = server.post("/v1/responses", &codex_second_turn("code", &item));
    drop(server);
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.contains("event: response.completed"), "{second}");
    let seen = upstream.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let sent = request_body(&seen[1]);
    let messages = sent["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap_or_else(|| panic!("no assistant turn reached the upstream: {sent}"));
    let blocks = assistant["content"].as_array().unwrap();
    assert_eq!(
        blocks[0],
        serde_json::json!({"type": "thinking", "thinking": "Read the file.", "signature": "sig-upstream-1"}),
        "{sent}"
    );
    assert_eq!(blocks[1]["type"], "tool_use", "{sent}");
    assert_eq!(blocks[1]["id"], "call_capture_1", "{sent}");
    assert!(
        sent.to_string()
            .contains("\"tool_use_id\":\"call_capture_1\""),
        "{sent}"
    );
    assert!(!sent.to_string().contains(SEAL_PREFIX), "{sent}");
}

/// Codex's recorded two-turn exchange through a responses-to-chat route: a chat upstream mints
/// no state, so the reasoning it showed is finished as an item sealed over nothing, and the
/// second turn replaying it is relayed with that item dropped and the call and its result
/// carried.
#[test]
fn translation_codexs_two_turns_reach_a_chat_upstream_with_the_sealed_item_dropped() {
    let fixture = Fixture::new("translation-codex-two-turns-chat");
    let upstream = Upstream::script(vec![CHAT_REASONING_CALL, CHAT_ANSWER]);
    let server = Server::start(&document(
        &fixture,
        &hosted_model("code", "chat", &upstream),
    ));
    let first = server.post("/v1/responses", CODEX_FIRST_TURN);
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    let item = reasoning_item(&first);
    assert!(
        item["encrypted_content"]
            .as_str()
            .is_some_and(|sealed| sealed.starts_with(SEAL_PREFIX)),
        "{item}"
    );
    let second = server.post("/v1/responses", &codex_second_turn("code", &item));
    drop(server);
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    let seen = upstream.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let sent = request_body(&seen[1]);
    let text = sent.to_string();
    for absent in [SEAL_PREFIX, "Read the file."] {
        assert!(
            !text.contains(absent),
            "{absent} reached a chat upstream: {text}"
        );
    }
    let messages = sent["messages"].as_array().unwrap();
    // Codex's `reasoning.effort` reaches the upstream as `reasoning_effort`; no message carries
    // reasoning.
    for message in messages {
        let message = message.to_string();
        assert!(
            !message.contains("reasoning"),
            "reasoning reached a chat upstream: {message}"
        );
    }
    let call = messages
        .iter()
        .find(|message| message.get("tool_calls").is_some())
        .unwrap_or_else(|| panic!("no tool call reached the upstream: {sent}"));
    assert_eq!(call["tool_calls"][0]["id"], "call_capture_1", "{sent}");
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap_or_else(|| panic!("no tool result reached the upstream: {sent}"));
    assert_eq!(result["tool_call_id"], "call_capture_1", "{sent}");
}

/// Whether `answer` is the gateway's `translation-opaque-state` refusal.
fn refused_opaque_state(answer: &str) -> bool {
    answer.starts_with("HTTP/1.1 400 ") && answer.contains("\"code\":\"translation-opaque-state\"")
}

/// A replayed item binds only on the route that sealed it, unaltered, while the gateway that
/// sealed it still runs: the recorded turn's own item (sealed by no gateway), the sealed item
/// with one character of its state altered, the sealed item sent to another route, and the
/// sealed item sent after the gateway restarted are each `translation-opaque-state`, and none
/// asks for a target. The refusal carries neither the seal nor the state. Without its reasoning
/// item, the recorded turn is carried.
#[test]
fn translation_a_replayed_item_the_gateway_did_not_seal_for_this_route_is_refused() {
    for upstream in [Wire::Chat, Wire::Messages] {
        assert_eq!(
            prepared(Wire::Responses, upstream, CODEX_SECOND_TURN.as_bytes()),
            Some(RefusalCode::TranslationOpaqueState),
            "Codex's recorded second turn to {upstream:?}"
        );
    }
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_SECOND_TURN).unwrap();
    codex["input"]
        .as_array_mut()
        .unwrap()
        .retain(|item| item["type"] != "reasoning");
    for upstream in [Wire::Chat, Wire::Messages] {
        assert_eq!(
            prepared(Wire::Responses, upstream, codex.to_string().as_bytes()),
            None,
            "Codex's second turn without its reasoning item to {upstream:?}"
        );
    }

    let fixture = Fixture::new("translation-sealed-refusals");
    let code = Upstream::script(vec![MESSAGES_THINKING_CALL]);
    let other = Upstream::script(Vec::new());
    let config = document(
        &fixture,
        &format!(
            "{}{}",
            hosted_model("code", "messages", &code),
            hosted_model("other", "messages", &other)
        ),
    );
    let server = Server::start(&config);
    let first = server.post("/v1/responses", CODEX_FIRST_TURN);
    let item = reasoning_item(&first);
    let sealed = item["encrypted_content"].as_str().unwrap().to_owned();
    let mut altered = item.clone();
    let (head, state) = sealed.split_at(SEAL_PREFIX.len());
    let flipped = if state.starts_with('A') { 'B' } else { 'A' };
    altered["encrypted_content"] = serde_json::json!(format!("{head}{flipped}{}", &state[1..]));
    let mut answers = vec![
        ("recorded", server.post("/v1/responses", CODEX_SECOND_TURN)),
        (
            "altered",
            server.post("/v1/responses", &codex_second_turn("code", &altered)),
        ),
        (
            "another route",
            server.post("/v1/responses", &codex_second_turn("other", &item)),
        ),
    ];
    drop(server);
    let restarted = Server::start(&config);
    answers.push((
        "after a restart",
        restarted.post("/v1/responses", &codex_second_turn("code", &item)),
    ));
    drop(restarted);
    for (name, answer) in &answers {
        assert!(refused_opaque_state(answer), "{name}: {answer}");
        assert!(!answer.contains(state), "{name}: {answer}");
        assert!(!answer.contains("sig-upstream-1"), "{name}: {answer}");
    }
    assert_eq!(code.seen().len(), 1);
    assert!(other.seen().is_empty());
}

/// No line the binary logs carries a seal, the state it holds or the upstream's signature, at
/// the most verbose level, through a sealed answer, a bound replay and a refused one.
#[test]
fn translation_no_log_line_carries_a_seal_or_the_upstream_state() {
    let fixture = Fixture::new("translation-sealed-logs");
    let upstream = Upstream::script(vec![MESSAGES_THINKING_CALL, MESSAGES_ANSWER]);
    let server = Server::start_logging(
        &document(&fixture, &hosted_model("code", "messages", &upstream)),
        "trace",
    );
    let first = server.post("/v1/responses", CODEX_FIRST_TURN);
    let item = reasoning_item(&first);
    let sealed = item["encrypted_content"].as_str().unwrap().to_owned();
    let bound = server.post("/v1/responses", &codex_second_turn("code", &item));
    assert!(bound.starts_with("HTTP/1.1 200 OK\r\n"), "{bound}");
    let mut altered = item.clone();
    altered["encrypted_content"] = serde_json::json!(format!("{sealed}A"));
    let refused = server.post("/v1/responses", &codex_second_turn("code", &altered));
    assert!(refused_opaque_state(&refused), "{refused}");
    let log = server.log();
    let state = sealed[SEAL_PREFIX.len()..].split('.').next().unwrap();
    for line in &log {
        for secret in [sealed.as_str(), state, "sig-upstream-1", SEAL_PREFIX] {
            assert!(!line.contains(secret), "{secret} was logged: {line}");
        }
    }
    assert_eq!(upstream.seen().len(), 2);
}

/// What llm 0.13.0 still refuses, it refuses by name, and the gateway answers each
/// `translation-unsupported` before asking for a target: Claude Code's structured-output turn
/// (`output_config.format`), `thinking.display`, `context_management` and `safeguards`, and a
/// message of a role outside the subset (`tool`; llm 0.19.0 reads `system`); on Responses,
/// `text.format` and a hosted tool.
#[test]
fn translation_fields_llm_still_refuses_are_refused_by_name() {
    let base: serde_json::Value =
        serde_json::from_str(&fixture_body(CLAUDE_CODE_FIRST_TURN)).unwrap();
    let structured: serde_json::Value = serde_json::from_str(&fixture_body(include_str!(
        "../../llm-gateway/tests/fixtures/claude-code/3-structured-output.request.txt"
    )))
    .unwrap();
    let mut display = base.clone();
    display["thinking"]["display"] = serde_json::json!("summarized");
    let mut context = base.clone();
    context["context_management"] =
        serde_json::json!({"edits": [{"type": "clear_tool_uses_20250919"}]});
    let mut safeguards = base.clone();
    safeguards["safeguards"] = serde_json::json!({});
    let mut role = base.clone();
    role["messages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"role": "tool", "content": "Be brief."}));
    for (name, body) in [
        ("output_config.format", &structured),
        ("display", &display),
        ("context_management", &context),
        ("safeguards", &safeguards),
        ("Messages role is outside the declared subset", &role),
    ] {
        let (code, message) = messages_refusal(body);
        assert_eq!(code, llm_core::ErrorCode::Unsupported, "{name}: {message}");
        assert!(message.contains(name), "{name}: {message}");
        for upstream in [Wire::Chat, Wire::Responses] {
            assert_eq!(
                prepared(Wire::Messages, upstream, body.to_string().as_bytes()),
                Some(RefusalCode::TranslationUnsupported),
                "{name} to {upstream:?}"
            );
        }
    }
    let codex: serde_json::Value = serde_json::from_str(CODEX_FIRST_TURN).unwrap();
    let mut format = codex.clone();
    format["text"] = serde_json::json!({"format": {"type": "json_schema", "name": "answer", "schema": {"type": "object"}}});
    let mut hosted = codex;
    hosted["tools"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type": "web_search"}));
    for (name, body) in [("text.format", &format), ("web_search", &hosted)] {
        for upstream in [Wire::Chat, Wire::Messages] {
            assert_eq!(
                prepared(Wire::Responses, upstream, body.to_string().as_bytes()),
                Some(RefusalCode::TranslationUnsupported),
                "{name} to {upstream:?}"
            );
        }
    }
}

/// llm 0.13.0 checks the effort the upstream wire receives: effort `max` reaches a chat or
/// responses upstream as `xhigh`, and disabled thinking as `none`, both of which the gateway
/// admits there; a messages upstream receives `max` and `none` as they are, and is not
/// translated either. A thinking budget has no name on chat or responses.
#[test]
fn translation_an_effort_is_checked_as_the_upstream_wire_receives_it() {
    let messages = |extra: &str| {
        format!(
            "{{\"model\":\"code\",\"max_tokens\":64,\"messages\":[{{\"role\":\"user\",\"content\":\"Hi\"}}]{extra}}}"
        )
    };
    let chat = |extra: &str| {
        format!(
            "{{\"model\":\"code\",\"messages\":[{{\"role\":\"user\",\"content\":\"Hi\"}}]{extra}}}"
        )
    };
    for upstream in [Wire::Chat, Wire::Responses] {
        for (extra, refused) in [
            (",\"output_config\":{\"effort\":\"max\"}", None),
            (",\"thinking\":{\"type\":\"disabled\"}", None),
            (",\"thinking\":{\"type\":\"adaptive\"}", None),
            (
                ",\"thinking\":{\"type\":\"enabled\",\"budget_tokens\":1024}",
                Some(RefusalCode::TranslationUnsupported),
            ),
            (
                ",\"thinking\":{\"type\":\"disabled\"},\"output_config\":{\"effort\":\"low\"}",
                Some(RefusalCode::TranslationUnsupported),
            ),
        ] {
            assert_eq!(
                prepared(Wire::Messages, upstream, messages(extra).as_bytes()),
                refused,
                "{extra} to {upstream:?}"
            );
        }
    }
    for effort in ["max", "none"] {
        assert_eq!(
            prepared(
                Wire::Chat,
                Wire::Messages,
                chat(&format!(",\"reasoning_effort\":\"{effort}\"")).as_bytes()
            ),
            Some(RefusalCode::TranslationUnsupported),
            "{effort} to a messages upstream"
        );
    }
    assert_eq!(
        prepared(
            Wire::Chat,
            Wire::Responses,
            chat(",\"reasoning_effort\":\"none\"").as_bytes()
        ),
        None
    );
}

/// The capabilities a model is translated under: tools when it parses tool calls, sampling,
/// efforts `minimal` to `xhigh`, and an output limit no larger than its window (200000 when it
/// declares none); an override replaces them for its alias.
#[test]
fn translation_a_model_is_translated_under_its_declared_capabilities() {
    let request = |extra: &str| {
        format!(
            "{{\"model\":\"code\",\"messages\":[{{\"role\":\"user\",\"content\":\"Hi\"}}]{extra}}}"
        )
    };
    let translator = LlmTranslator::new();
    let windowed = relay_model("code", vec![Wire::Messages]).with_max_model_len(4096);
    let prepare = |translator: &LlmTranslator, model: &RelayModel, body: &str| {
        translator
            .prepare(&TranslationRequest {
                ingress: Wire::Chat,
                upstream: Wire::Messages,
                model,
                body: body.as_bytes(),
                anthropic_beta: None,
                caller: llm_gateway::Caller::default(),
            })
            .err()
    };
    for (extra, refused) in [
        ("", None),
        (",\"temperature\":0.5,\"top_p\":0.9", None),
        (",\"reasoning_effort\":\"minimal\"", None),
        (",\"reasoning_effort\":\"xhigh\"", None),
        (
            ",\"reasoning_effort\":\"max\"",
            Some(RefusalCode::TranslationUnsupported),
        ),
        (",\"max_tokens\":4096", None),
        (
            ",\"max_tokens\":4097",
            Some(RefusalCode::TranslationUnsupported),
        ),
        (
            ",\"temperature\":1.5",
            Some(RefusalCode::TranslationUnsupported),
        ),
    ] {
        assert_eq!(
            prepare(&translator, &windowed, &request(extra)),
            refused,
            "{extra}"
        );
    }
    let unwindowed = relay_model("code", vec![Wire::Messages]);
    assert_eq!(
        prepare(&translator, &unwindowed, &request(",\"max_tokens\":200000")),
        None
    );
    assert_eq!(
        prepare(&translator, &unwindowed, &request(",\"max_tokens\":200001")),
        Some(RefusalCode::TranslationUnsupported)
    );
    let tools = ",\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"parameters\":{\"type\":\"object\"}}}]";
    let absent = windowed.clone().with_tool_calling(ToolCalling::Absent);
    assert_eq!(
        prepare(&translator, &absent, &request(tools)),
        Some(RefusalCode::TranslationUnsupported)
    );
    assert_eq!(prepare(&translator, &windowed, &request(tools)), None);
    let narrowed =
        LlmTranslator::new().with_capabilities("code", llm_core::Capabilities::text(1024, 512));
    assert_eq!(
        prepare(&narrowed, &windowed, &request(",\"max_tokens\":513")),
        Some(RefusalCode::TranslationUnsupported)
    );
    assert_eq!(
        prepare(&narrowed, &windowed, &request(",\"max_tokens\":512")),
        None
    );
}

/// story:translated-refusal-target-status: a translated call the target answered `429` is
/// refused `upstream-refused`, and its usage record carries the target's status, as a relayed
/// call's does. The route counters it feeds are held by
/// `crates/llm-gateway/tests/translation_port.rs`: this chat request for a messages-only model
/// is on a wire the model does not declare, so it feeds the process-wide series only.
#[test]
fn translation_a_call_the_target_answered_429_records_the_target_status() {
    let fixture = Fixture::new("target-status-429");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection.set_read_timeout(Some(WAIT)).unwrap();
        drop(read_request(&mut connection));
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let head = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        drop(connection.write_all(head.as_bytes()));
        drop(connection.write_all(body.as_bytes()));
    });
    let table = format!(
        "[hosted.code]\nwires = [\"messages\"]\nbase_url = \"http://{address}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n"
    );
    let server = Server::start(&document(&fixture, &table));
    let body = r#"{"model":"code","messages":[{"role":"user","content":"hi"}]}"#;
    let answer = server.post("/v1/chat/completions", body);
    upstream.join().unwrap();
    assert!(answer.contains("\"upstream-refused\""), "{answer}");
    let log = server.log();
    let records: Vec<&String> = log
        .iter()
        .filter(|line| line.contains("usage record"))
        .collect();
    let [record] = records.as_slice() else {
        panic!("expected one usage record: {log:#?}");
    };
    assert!(record.contains("target_status=429"), "{record}");
}

// --- A refusal names the refused path in a log line -------------------------------------------

/// A value no log line may carry. It is written into the request's values: message text, a
/// tool's name and description, a call's name, arguments and result, and the refused value
/// itself where llm refuses a value.
const LOG_MARKER: &str = "zqRefusalMarker7x";

/// One request llm's decoder refuses: the code it is answered with and the path or field name
/// the log line names.
struct Refused {
    body: serde_json::Value,
    code: RefusalCode,
    names: &'static str,
}

/// `base` with the JSON `value` written at `pointer`'s `key`.
fn refused_with(
    base: &serde_json::Value,
    pointer: &str,
    key: &str,
    value: serde_json::Value,
    code: RefusalCode,
    names: &'static str,
) -> Refused {
    let mut body = base.clone();
    body.pointer_mut(pointer).unwrap()[key] = value;
    Refused { body, code, names }
}

/// The lines of `log` that are a translation refusal's.
fn refusal_lines(log: &[String]) -> Vec<&String> {
    log.iter()
        .filter(|line| line.contains("translation refused"))
        .collect()
}

/// Posts each of `cases` on `ingress`'s path to the shipped binary serving `code` from an
/// endpoint speaking only `upstream`, and asserts: each answer is the gateway's refusal body for
/// its code, which names nothing llm said; the binary logs one `info` line per refusal, in order,
/// naming its code, the client's wire and the path or field name llm refused; and the marker is
/// in no log line and no answer. No request reaches the endpoint.
fn assert_each_refusal_names_its_path(
    name: &str,
    ingress: Wire,
    upstream: &str,
    cases: &[Refused],
) {
    let fixture = Fixture::new(name);
    let endpoint = Upstream::script(Vec::new());
    let server = Server::start(&document(
        &fixture,
        &hosted_model("code", upstream, &endpoint),
    ));
    for case in cases {
        let answer = server.post(ingress.path(), &case.body.to_string());
        assert!(
            answer.starts_with("HTTP/1.1 400 "),
            "{}: {answer}",
            case.names
        );
        let (_, body) = answer.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            serde_json::json!({"error": {"code": case.code.wire(), "message": case.code.reason()}}),
            "{}",
            case.names
        );
        assert!(!answer.contains(LOG_MARKER), "{}: {answer}", case.names);
    }
    let log = server.log();
    for line in &log {
        assert!(!line.contains(LOG_MARKER), "the marker was logged: {line}");
    }
    let refusals = refusal_lines(&log);
    assert_eq!(refusals.len(), cases.len(), "{log:#?}");
    for (line, case) in refusals.iter().zip(cases) {
        assert!(line.contains(" INFO "), "{line}");
        assert!(
            line.contains(&format!("refusal={}", case.code.wire())),
            "{line}"
        );
        assert!(
            line.contains(&format!("ingress={}", ingress.label())),
            "{line}"
        );
        assert!(line.contains("model=\"code\""), "{line}");
        assert!(line.contains(case.names), "{}: {line}", case.names);
    }
    assert!(endpoint.seen().is_empty());
}

/// A chat client's request with the marker in its values.
fn marked_chat() -> serde_json::Value {
    let arguments = serde_json::json!({ "city": LOG_MARKER }).to_string();
    serde_json::json!({
        "model": "code",
        "messages": [
            {"role": "system", "content": "Be brief."},
            {"role": "user", "content": [{"type": "text", "text": LOG_MARKER}]},
            {"role": "assistant", "content": "Checking.", "tool_calls": [
                {"id": "call_1", "type": "function", "function": {"name": LOG_MARKER, "arguments": arguments}}
            ]},
            {"role": "tool", "tool_call_id": "call_1", "content": LOG_MARKER}
        ],
        "tools": [{"type": "function", "function": {"name": LOG_MARKER, "description": LOG_MARKER, "parameters": {"type": "object"}}}],
        "stream": true
    })
}

/// story:translation-refusal-names-path, chat: a key outside the subset at a nested level and at
/// the top, a key whose name is not of the wire's shape (written `?`), a role and a value of the
/// wrong kind are each refused as before and logged once by path or field, never by value.
#[test]
fn translation_a_chat_refusal_names_the_refused_path_in_one_log_line() {
    let base = marked_chat();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "/messages/1/content/0",
            "cache_control",
            serde_json::json!({"type": "ephemeral"}),
            unsupported,
            "messages[].content[].?\"",
        ),
        refused_with(
            &base,
            "",
            "safeguards",
            serde_json::json!({"note": LOG_MARKER}),
            unsupported,
            "outside the published subset: ?\"",
        ),
        refused_with(
            &base,
            "",
            &format!("{LOG_MARKER} key"),
            serde_json::json!(true),
            unsupported,
            "outside the published subset: ?",
        ),
        refused_with(
            &base,
            "/messages/1",
            "role",
            serde_json::json!(LOG_MARKER),
            unsupported,
            "chat message role",
        ),
        refused_with(
            &base,
            "",
            "temperature",
            serde_json::json!(LOG_MARKER),
            RefusalCode::TranslationInvalid,
            "`temperature` is not a number",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-log-chat", Wire::Chat, "messages", &cases);
}

/// A responses client's request with the marker in its values.
fn marked_responses() -> serde_json::Value {
    let arguments = serde_json::json!({ "city": LOG_MARKER }).to_string();
    let message = |role: &str, part: &str, text: &str| serde_json::json!({"type": "message", "role": role, "content": [{"type": part, "text": text}]});
    serde_json::json!({
        "model": "code",
        "input": [
            message("developer", "input_text", "Be brief."),
            message("user", "input_text", LOG_MARKER),
            {"type": "function_call", "call_id": "call_1", "name": LOG_MARKER, "arguments": arguments},
            {"type": "function_call_output", "call_id": "call_1", "output": LOG_MARKER},
            message("user", "input_text", "Thanks")
        ],
        "tools": [{"type": "function", "name": LOG_MARKER, "description": LOG_MARKER, "parameters": {"type": "object"}, "strict": false}],
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"]
    })
}

/// story:translation-refusal-names-path, responses: `text.format`, a role and a value of the
/// wrong kind are each refused as before and logged once by path or field, never by value.
#[test]
fn translation_a_responses_refusal_names_the_refused_path_in_one_log_line() {
    let base = marked_responses();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "",
            "text",
            serde_json::json!({"format": {"type": "json_schema", "name": LOG_MARKER, "schema": {"type": "object"}}}),
            unsupported,
            "text.format",
        ),
        refused_with(
            &base,
            "/input/1",
            "role",
            serde_json::json!(LOG_MARKER),
            unsupported,
            "message role outside the pinned subset",
        ),
        refused_with(
            &base,
            "",
            "temperature",
            serde_json::json!(LOG_MARKER),
            RefusalCode::TranslationInvalid,
            "`temperature` is not a number",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-log-responses", Wire::Responses, "chat", &cases);
}

/// A messages client's request with the marker in its values.
fn marked_messages() -> serde_json::Value {
    serde_json::json!({
        "model": "code",
        "max_tokens": 64,
        "system": [{"type": "text", "text": "Be brief."}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": LOG_MARKER}]},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "toolu_1", "name": LOG_MARKER, "input": {"city": LOG_MARKER}}
            ]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": LOG_MARKER, "is_error": false}]}
        ],
        "tools": [{"name": LOG_MARKER, "description": LOG_MARKER, "input_schema": {"type": "object"}}],
        "stream": true
    })
}

/// story:translation-refusal-names-path, messages: Claude Code's `context_management` and
/// `thinking.display`, a `cache_control` key outside its subset, a role and a value of the wrong
/// kind are each refused as before and logged once by path or field, never by value.
#[test]
fn translation_a_messages_refusal_names_the_refused_path_in_one_log_line() {
    let base = marked_messages();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "",
            "context_management",
            serde_json::json!({"edits": [{"type": LOG_MARKER}]}),
            unsupported,
            "context_management",
        ),
        refused_with(
            &base,
            "",
            "thinking",
            serde_json::json!({"type": "adaptive", "display": LOG_MARKER}),
            unsupported,
            "thinking.display",
        ),
        refused_with(
            &base,
            "/system/0",
            "cache_control",
            serde_json::json!({"type": "ephemeral", "scope": LOG_MARKER}),
            unsupported,
            "system[].cache_control.?\"",
        ),
        refused_with(
            &base,
            "/messages/0",
            "role",
            serde_json::json!(LOG_MARKER),
            unsupported,
            "Messages role is outside the declared subset",
        ),
        refused_with(
            &base,
            "",
            "stream",
            serde_json::json!(LOG_MARKER),
            RefusalCode::TranslationInvalid,
            "Messages stream must be boolean",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-log-messages", Wire::Messages, "chat", &cases);
}

/// A key a client chose shaped as a secret: 64 bytes of `[A-Za-z0-9_-]`, the longest name llm
/// copies into a path, holding the marker.
const SECRET_KEY: &str = "sk_live_zqRefusalMarker7x_0123456789abcdefghijklmnopqrstuvwxyzAB";

#[test]
fn translation_the_secret_shaped_key_is_one_llm_would_copy() {
    assert_eq!(SECRET_KEY.len(), 64);
    assert!(
        SECRET_KEY
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );
    assert!(SECRET_KEY.contains(LOG_MARKER));
}

/// story:refusal-log-masks-client-keys, chat: a secret-shaped key outside the subset at the
/// root, in a message and in a content part is logged as `?` at its location, never by name;
/// `n`, a name from llm's own list, is still named.
#[test]
fn translation_a_chat_refusal_writes_a_clients_key_as_a_question_mark() {
    let base = marked_chat();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"chat request field is outside the published subset: ?\"",
        ),
        refused_with(
            &base,
            "/messages/1",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"chat request field is outside the published subset: messages[].?\"",
        ),
        refused_with(
            &base,
            "/messages/1/content/0",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"chat request field is outside the published subset: messages[].content[].?\"",
        ),
        refused_with(
            &base,
            "",
            "n",
            serde_json::json!(2),
            unsupported,
            "refused=\"chat request field `n` is outside the published subset\"",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-mask-chat", Wire::Chat, "messages", &cases);
}

/// story:refusal-log-masks-client-keys, responses: a secret-shaped key at the root, in `text`
/// and in a content part is logged as `?` at its location; `text.format`, a name the contract
/// publishes as refused by name, is still named.
#[test]
fn translation_a_responses_refusal_writes_a_clients_key_as_a_question_mark() {
    let base = marked_responses();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"request body carries a field outside the pinned subset: ?\"",
        ),
        refused_with(
            &base,
            "",
            "text",
            serde_json::json!({ SECRET_KEY: true }),
            unsupported,
            "refused=\"request body carries a field outside the pinned subset: text.?\"",
        ),
        refused_with(
            &base,
            "/input/1/content/0",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"request body carries a field outside the pinned subset: input[].content[].?\"",
        ),
        refused_with(
            &base,
            "",
            "text",
            serde_json::json!({"format": {"type": "json_schema", "name": "answer", "schema": {"type": "object"}}}),
            unsupported,
            "refused=\"request body carries a field outside the pinned subset: text.format\"",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-mask-responses", Wire::Responses, "chat", &cases);
}

/// story:refusal-log-masks-client-keys, messages: a secret-shaped key at the root, in
/// `thinking` and in a content block is logged as `?` at its location; `thinking.display` and
/// `context_management`, names the contract publishes as refused by name, are still named.
#[test]
fn translation_a_messages_refusal_writes_a_clients_key_as_a_question_mark() {
    let base = marked_messages();
    let unsupported = RefusalCode::TranslationUnsupported;
    let cases = [
        refused_with(
            &base,
            "",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"Messages field is outside the declared subset: ?\"",
        ),
        refused_with(
            &base,
            "",
            "thinking",
            serde_json::json!({"type": "adaptive", SECRET_KEY: true}),
            unsupported,
            "refused=\"Messages field is outside the declared subset: thinking.?\"",
        ),
        refused_with(
            &base,
            "/messages/0/content/0",
            SECRET_KEY,
            serde_json::json!(true),
            unsupported,
            "refused=\"Messages field is outside the declared subset: messages[].content[].?\"",
        ),
        refused_with(
            &base,
            "",
            "thinking",
            serde_json::json!({"type": "adaptive", "display": "summarized"}),
            unsupported,
            "refused=\"Messages field is outside the declared subset: thinking.display\"",
        ),
        refused_with(
            &base,
            "",
            "context_management",
            serde_json::json!({"edits": []}),
            unsupported,
            "refused=\"Messages field is outside the declared subset: context_management\"",
        ),
    ];
    assert_each_refusal_names_its_path("refusal-mask-messages", Wire::Messages, "chat", &cases);
}
