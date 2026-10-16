//! story:gateway-translation, what the conformance program cannot reach: the shipped binary
//! always composes its translator; a translated request reaches a hosted endpoint on its base
//! path with its credential as its kind says and nothing else of the gateway's; a hosted target
//! reports its scheme; and the known limits of llm 0.6.0's translated subset, each pinned by
//! feeding the request to the decoder the gateway uses.
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

/// Known limit: Claude Code's first turn (the authored fixture of `crates/llm-gateway`) is
/// outside llm 0.6.0's messages subset, field after field: `thinking`, a `system` of more than
/// one block, `cache_control` on a content block, and then effort `max`, which no capability
/// names. The gateway answers each `translation-unsupported`.
#[test]
fn translation_known_limit_claude_codes_first_turn_is_outside_the_messages_subset() {
    let body: serde_json::Value = serde_json::from_str(&fixture_body(include_str!(
        "../../llm-gateway/tests/fixtures/claude-code/1-tool-turn.request.txt"
    )))
    .unwrap();
    let origin = llm_core::Provenance {
        protocol: llm_core::Protocol::Messages,
        provider: llm_core::Id::new("client").unwrap(),
        account: llm_core::Id::new("client").unwrap(),
        endpoint: llm_core::Id::new("ingress").unwrap(),
        model: llm_core::Id::new("code").unwrap(),
        binding_revision: llm_core::Id::new("ingress").unwrap(),
    };
    let refused = |body: &serde_json::Value| {
        let error = llm_messages::decode_request(body.to_string().as_bytes(), &origin).unwrap_err();
        (error.code, error.message)
    };
    let mut body = body;
    assert_eq!(
        refused(&body),
        (
            llm_core::ErrorCode::Unsupported,
            "Messages field is outside the declared subset: thinking".to_owned()
        )
    );
    body.as_object_mut().unwrap().remove("thinking");
    assert_eq!(
        refused(&body),
        (
            llm_core::ErrorCode::Unsupported,
            "Messages system shape is outside the declared subset".to_owned()
        )
    );
    body.as_object_mut().unwrap().remove("system");
    assert_eq!(
        refused(&body),
        (
            llm_core::ErrorCode::Unsupported,
            "Messages field is outside the declared subset: messages[].content[].cache_control"
                .to_owned()
        )
    );
    body["messages"][0]["content"][0]
        .as_object_mut()
        .unwrap()
        .remove("cache_control");
    let decoded = llm_messages::decode_request(body.to_string().as_bytes(), &origin).unwrap();
    assert_eq!(
        decoded.request.sampling.reasoning_effort.as_deref(),
        Some("max")
    );
    for upstream in [Wire::Chat, Wire::Responses] {
        assert_eq!(
            prepared(Wire::Messages, upstream, body.to_string().as_bytes()),
            Some(RefusalCode::TranslationUnsupported),
            "effort max to {upstream:?}"
        );
    }
    body["output_config"]["effort"] = serde_json::json!("high");
    for upstream in [Wire::Chat, Wire::Responses] {
        assert_eq!(
            prepared(Wire::Messages, upstream, body.to_string().as_bytes()),
            None
        );
    }
}

/// Known limit: the top-level fields Codex sends beside the subset are refused by
/// `ingest_request` (`ACCEPTED_BODY_FIELDS`), and so is a `reasoning.summary`; the gateway
/// answers each `translation-unsupported`.
#[test]
fn translation_known_limit_codex_fields_are_outside_the_responses_subset() {
    let base = serde_json::json!({
        "model": "code",
        "input": [{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],
        "stream": true,
        "store": false,
        "include": ["reasoning.encrypted_content"],
    });
    let binding = llm_responses::Binding::new(
        llm_core::Provenance {
            protocol: llm_core::Protocol::Responses,
            provider: llm_core::Id::new("client").unwrap(),
            account: llm_core::Id::new("client").unwrap(),
            endpoint: llm_core::Id::new("ingress").unwrap(),
            model: llm_core::Id::new("code").unwrap(),
            binding_revision: llm_core::Id::new("ingress").unwrap(),
        },
        llm_core::Id::new("code").unwrap(),
    );
    assert!(llm_responses::ingest_request(&binding, &base).is_ok());
    assert_eq!(
        prepared(Wire::Responses, Wire::Chat, base.to_string().as_bytes()),
        None
    );
    for (field, value, message) in [
        (
            "instructions",
            serde_json::json!("Be brief."),
            "request body carries a field outside the pinned subset",
        ),
        (
            "parallel_tool_calls",
            serde_json::json!(false),
            "request body carries a field outside the pinned subset",
        ),
        (
            "prompt_cache_key",
            serde_json::json!("session-1"),
            "request body carries a field outside the pinned subset",
        ),
        (
            "text",
            serde_json::json!({"verbosity":"low"}),
            "request body carries a field outside the pinned subset",
        ),
        (
            "reasoning",
            serde_json::json!({"effort":"medium","summary":"auto"}),
            "`reasoning` carries a field outside the pinned subset",
        ),
    ] {
        let mut body = base.clone();
        body[field] = value;
        let error = llm_responses::ingest_request(&binding, &body).unwrap_err();
        assert_eq!(
            (error.code, error.message.as_str()),
            (llm_core::ErrorCode::Unsupported, message),
            "{field}"
        );
        assert_eq!(
            prepared(Wire::Responses, Wire::Chat, body.to_string().as_bytes()),
            Some(RefusalCode::TranslationUnsupported),
            "{field}"
        );
    }
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
