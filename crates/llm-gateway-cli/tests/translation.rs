//! story:gateway-translation, what the conformance program cannot reach: the shipped binary
//! always composes its translator; a translated request reaches a hosted endpoint on its base
//! path with its credential as its kind says and nothing else of the gateway's; a hosted target
//! reports its scheme; and what llm 0.7.0 does with Claude Code's and Codex's turns: carried and
//! relayed where it carries them, refused by name where it does not, and Codex's replayed
//! reasoning item still refused as carried state.
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
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let thread = thread::spawn(move || {
            for _ in 0..answers {
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
}

impl Server {
    fn start(config: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
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
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = lines.recv_timeout(left).unwrap_or_else(|_| {
                drop(child.kill());
                panic!("the binary never reported {LISTENING:?}");
            });
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
        };
        Self { child, address }
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

/// llm 0.7.0's messages decoder's refusal of `body`, read as a client's.
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

/// Codex's second turn replays the reasoning item its first answer carried, encrypted for the
/// upstream that minted it: a translated route still refuses it `translation-opaque-state`
/// before asking for a target. The rest of the turn, the function call and its plain-string
/// output, is carried.
#[test]
fn translation_codexs_second_turn_is_refused_for_its_reasoning_item_only() {
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_SECOND_TURN).unwrap();
    for upstream in [Wire::Chat, Wire::Messages] {
        assert_eq!(
            prepared(Wire::Responses, upstream, CODEX_SECOND_TURN.as_bytes()),
            Some(RefusalCode::TranslationOpaqueState),
            "Codex's second turn to {upstream:?}"
        );
    }
    let input = codex["input"].as_array_mut().unwrap();
    let before = input.len();
    input.retain(|item| item["type"] != "reasoning");
    assert_eq!(input.len(), before - 1);
    assert!(input.iter().any(|item| item["type"] == "function_call"));
    assert!(
        input
            .iter()
            .any(|item| item["type"] == "function_call_output")
    );
    for upstream in [Wire::Chat, Wire::Messages] {
        assert_eq!(
            prepared(Wire::Responses, upstream, codex.to_string().as_bytes()),
            None,
            "Codex's second turn without its reasoning item to {upstream:?}"
        );
    }
    let (_, sent) = through_the_binary(
        "translation-codex-second",
        "chat",
        CHAT_ANSWER,
        "/v1/responses",
        &codex.to_string(),
    );
    let messages = sent["messages"].as_array().unwrap();
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

/// What llm 0.7.0 still refuses, it refuses by name, and the gateway answers each
/// `translation-unsupported` before asking for a target: Claude Code's structured-output turn
/// (`output_config.format`), `thinking.display`, `context_management`, `safeguards` and a
/// `system` message inside `messages`; on Responses, `text.format` and a hosted tool.
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
    let mut midway = base.clone();
    midway["messages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"role": "system", "content": "Be brief."}));
    for (name, body) in [
        ("output_config.format", &structured),
        ("display", &display),
        ("context_management", &context),
        ("safeguards", &safeguards),
        ("system", &midway),
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

/// llm 0.7.0 checks the effort the upstream wire receives: effort `max` reaches a chat or
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
