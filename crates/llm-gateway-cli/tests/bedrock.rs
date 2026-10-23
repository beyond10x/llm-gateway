//! story:bedrock-hosted-target, through the shipped binary: a `[hosted.<alias>]` entry of
//! `kind = "bedrock"` is served on every ingress wire, translated by llm and sent with its
//! Bedrock client, signed with the credentials the container credentials endpoint answers for
//! the token file the environment names. Fields Bedrock refuses are dropped and logged by name,
//! and no prompt or answer text is logged. A document or an environment that cannot serve such
//! an entry refuses the start.
//!
//! Both Bedrock and the credentials endpoint are loopback fixtures; nothing leaves the machine,
//! no credential is real and the model id is not checked against AWS.

mod support;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use llm_bedrock::eventstream::{HeaderValue, Message, encode_message};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, hosted_document};

const PREFIX: &str = "b10x-llm-gateway: ";
const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

const URI_VARIABLE: &str = "AWS_CONTAINER_CREDENTIALS_FULL_URI";
const TOKEN_VARIABLE: &str = "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE";

/// The container authorization token the fixture writes, with the newline a mounted file ends in.
const CONTAINER_TOKEN: &str = "fake-container-authorization-token-0123456789";
const ACCESS_KEY_ID: &str = "AKIDFAKE0000000001";
const SECRET_ACCESS_KEY: &str = "fake-secret-access-key-for-a-loopback-test";
const SESSION_TOKEN: &str = "fake-session-token-for-a-loopback-test";

/// The example model id. Unverified against AWS; the fixture endpoint accepts any id.
const MODEL_ID: &str = "eu.anthropic.claude-haiku-5-5";
const REGION: &str = "eu-central-1";

/// Text the client sends and the fixture answers, which no log line may carry.
const PROMPT_MARKER: &str = "prompt-marker-what-is-the-weather-in-berlin";
const ANSWER_MARKER: &str = "answer-marker-sunny";

const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The most beta names a turn carries, llm 0.13.0's `llm_core::MAX_ANTHROPIC_BETAS`.
const MAX_ANTHROPIC_BETAS: usize = 32;

// --- Event-stream frames ------------------------------------------------------------------------

fn frame(headers: &[(&str, &str)], payload: Vec<u8>) -> Vec<u8> {
    encode_message(&Message {
        headers: headers
            .iter()
            .map(|(name, value)| ((*name).to_owned(), HeaderValue::String((*value).to_owned())))
            .collect(),
        payload,
    })
}

/// One `InvokeModel` `chunk` frame carrying the Messages `event`.
fn chunk(event: &Value) -> Vec<u8> {
    frame(
        &[
            (":event-type", "chunk"),
            (":content-type", "application/json"),
            (":message-type", "event"),
        ],
        json!({"bytes": STANDARD.encode(event.to_string())})
            .to_string()
            .into_bytes(),
    )
}

/// One Converse stream event of `kind`.
fn converse_event(kind: &str, payload: &Value) -> Vec<u8> {
    frame(
        &[
            (":event-type", kind),
            (":content-type", "application/json"),
            (":message-type", "event"),
        ],
        payload.to_string().into_bytes(),
    )
}

fn message_start() -> Value {
    json!({"type":"message_start","message":{"id":"msg_bedrock_fixture","type":"message",
        "role":"assistant","content":[],"model":"claude-fixture","stop_reason":null,
        "stop_sequence":null,"usage":{"input_tokens":20,"output_tokens":1}}})
}

fn message_end(stop_reason: &str, output_tokens: u64) -> Vec<Value> {
    vec![
        json!({"type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},
            "usage":{"output_tokens":output_tokens}}),
        json!({"type":"message_stop","amazon-bedrock-invocationMetrics":{"inputTokenCount":20,
            "outputTokenCount":output_tokens,"invocationLatency":12,"firstByteLatency":5}}),
    ]
}

/// An `InvokeModel` stream answering [`ANSWER_MARKER`] as text.
fn invoke_text() -> Vec<u8> {
    let mut events = vec![
        message_start(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":ANSWER_MARKER}}),
        json!({"type":"content_block_stop","index":0}),
    ];
    events.extend(message_end("end_turn", 3));
    events.iter().flat_map(chunk).collect()
}

/// An `InvokeModel` stream calling `get_weather` with `{"city":"Berlin"}` in two deltas.
fn invoke_tool_call() -> Vec<u8> {
    invoke_call("get_weather", "city")
}

/// An `InvokeModel` stream calling `tool` with `{"<argument>":"Berlin"}` in two deltas.
fn invoke_call(tool: &str, argument: &str) -> Vec<u8> {
    let mut events = vec![
        message_start(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use",
            "id":"toolu_fixture_1","name":tool,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta",
            "partial_json":format!("{{\"{argument}\":")}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta",
            "partial_json":"\"Berlin\"}"}}),
        json!({"type":"content_block_stop","index":0}),
    ];
    events.extend(message_end("tool_use", 9));
    events.iter().flat_map(chunk).collect()
}

/// A Converse stream answering [`ANSWER_MARKER`] as text.
fn converse_text() -> Vec<u8> {
    let mut body = converse_event("messageStart", &json!({"role": "assistant"}));
    body.extend(converse_event(
        "contentBlockDelta",
        &json!({"contentBlockIndex": 0, "delta": {"text": ANSWER_MARKER}}),
    ));
    body.extend(converse_event(
        "contentBlockStop",
        &json!({"contentBlockIndex": 0}),
    ));
    body.extend(converse_event(
        "messageStop",
        &json!({"stopReason": "end_turn"}),
    ));
    body.extend(converse_event(
        "metadata",
        &json!({"usage": {"inputTokens": 20, "outputTokens": 3, "totalTokens": 23}}),
    ));
    body
}

// --- Loopback fixtures -------------------------------------------------------------------------

/// Answers each connection, in order, with the next of its answers, and records every request.
/// A fixture nobody reaches records nothing; [`Recorder::seen`] does not wait for it.
struct Recorder {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Recorder {
    fn start(content_type: &'static str, answers: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        thread::spawn(move || {
            for answer in answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let request = read_request(&mut connection);
                recording.lock().unwrap().push(request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                drop(connection.write_all(head.as_bytes()));
                drop(connection.write_all(&answer));
            }
        });
        Self { address, seen }
    }

    /// A container credentials endpoint answering every one of `count` fetches with the fake
    /// keys, valid until 2099.
    fn credentials(count: usize) -> Self {
        let answer = json!({
            "AccessKeyId": ACCESS_KEY_ID,
            "SecretAccessKey": SECRET_ACCESS_KEY,
            "Token": SESSION_TOKEN,
            "Expiration": "2099-01-01T00:00:00Z",
        })
        .to_string()
        .into_bytes();
        Self::start("application/json", vec![answer; count])
    }

    fn seen(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|raw| String::from_utf8_lossy(raw).into_owned())
            .collect()
    }
}

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

fn body(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

// --- The shipped binary ------------------------------------------------------------------------

/// The environment the binary is started with: the credentials URI and the token file.
struct Environment {
    variables: Vec<(&'static str, String)>,
}

impl Environment {
    fn container(credentials: &Recorder, token: &Path) -> Self {
        Self {
            variables: vec![
                (
                    URI_VARIABLE,
                    format!("http://{}/v1/credentials", credentials.address),
                ),
                (TOKEN_VARIABLE, token.display().to_string()),
            ],
        }
    }

    fn none() -> Self {
        Self {
            variables: Vec::new(),
        }
    }

    fn command(&self, config: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"));
        command
            .args(["--config", config.to_str().unwrap()])
            .env_remove(URI_VARIABLE)
            .env_remove(TOKEN_VARIABLE)
            .env_remove("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
            .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (name, value) in &self.variables {
            command.env(name, value);
        }
        command
    }
}

struct Server {
    child: Child,
    address: SocketAddr,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn start(config: &Path, environment: &Environment) -> Self {
        let mut child = environment.command(config).spawn().unwrap();
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
        let mut before = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                panic!("the binary never reported {LISTENING:?}: {before:?}");
            };
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
            before.push(line);
        };
        Self {
            child,
            address,
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

    fn exchange(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        String::from_utf8_lossy(&raw).into_owned()
    }

    fn post(&self, path: &str, body: &str) -> String {
        self.exchange(&format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ))
    }

    fn get(&self, path: &str) -> String {
        self.exchange(&format!(
            "GET {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        ))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Runs the binary to its exit and returns its exit code and standard error.
fn refused_with(config: &Path, environment: &Environment) -> (Option<i32>, String) {
    let mut child = environment.command(config).spawn().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let reader = thread::spawn(move || {
        let mut text = String::new();
        drop(stderr.read_to_string(&mut text));
        text
    });
    let deadline = Instant::now() + STARTUP;
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

fn assert_refused(config: &Path, environment: &Environment, code: &str) -> String {
    let (status, stderr) = refused_with(config, environment);
    let line = format!("{PREFIX}refused {code}: ");
    assert!(
        stderr.lines().any(|candidate| candidate.starts_with(&line)),
        "expected a line starting {line:?}; stderr: {stderr:?}"
    );
    assert_eq!(status, Some(1), "{stderr:?}");
    stderr
}

/// A test's fixture directory, its token file and its deployment document.
struct Deployment {
    fixture: Fixture,
    token: PathBuf,
}

impl Deployment {
    fn new(name: &str) -> Self {
        let fixture = Fixture::new(name);
        let token = fixture.write(
            "container-token",
            format!("{CONTAINER_TOKEN}\n").as_bytes(),
            0o644,
        );
        Self { fixture, token }
    }

    fn config(&self, tables: &str) -> PathBuf {
        let secret = self.fixture.owner_secret();
        self.fixture
            .config(&hosted_document("127.0.0.1:0", &secret, tables))
    }
}

/// A Bedrock entry `alias` for `model_id` at the loopback `bedrock` fixture, with `lines` after
/// its keys.
fn bedrock_table(alias: &str, model_id: &str, bedrock: &Recorder, lines: &str) -> String {
    format!(
        "[hosted.{alias}]\nkind = \"bedrock\"\nregion = \"{REGION}\"\nmodel_id = \"{model_id}\"\ncredential_source = \"container\"\nendpoint = \"http://{}\"\n{lines}\n",
        bedrock.address
    )
}

/// The weather tool on the chat wire.
fn chat_tools() -> Value {
    json!([{"type":"function","function":{"name":"get_weather","description":"Weather by city",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}])
}

/// Asserts the request reached Bedrock signed with the fake keys for the region and the
/// `bedrock` service, with the session token, and never the owner's credential.
fn assert_signed(request: &str) {
    let authorization = header(request, "authorization").unwrap_or_default();
    let prefix = format!("AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/");
    assert!(authorization.starts_with(&prefix), "{request}");
    assert!(
        authorization.contains(&format!("/{REGION}/bedrock/aws4_request")),
        "{request}"
    );
    assert_eq!(
        header(request, "x-amz-security-token"),
        Some(SESSION_TOKEN),
        "{request}"
    );
    assert!(!request.contains(OWNER_SECRET), "{request}");
}

/// Asserts the credentials endpoint was asked with the token file's token as `authorization`.
fn assert_fetched(credentials: &Recorder) {
    let seen = credentials.seen();
    assert!(!seen.is_empty(), "the credentials endpoint was never asked");
    for request in &seen {
        assert!(
            request.starts_with("GET /v1/credentials HTTP/1.1\r\n"),
            "{request}"
        );
        assert_eq!(
            header(request, "authorization"),
            Some(CONTAINER_TOKEN),
            "{request}"
        );
    }
}

/// Asserts no line the binary wrote carries prompt or answer text, or a secret.
fn assert_quiet(log: &[String]) {
    for line in log {
        for withheld in [
            PROMPT_MARKER,
            ANSWER_MARKER,
            CONTAINER_TOKEN,
            SECRET_ACCESS_KEY,
            SESSION_TOKEN,
            OWNER_SECRET,
            "Berlin",
        ] {
            assert!(!line.contains(withheld), "{withheld:?} was logged: {line}");
        }
    }
}

/// The log lines that name `field` as dropped for a Bedrock target.
fn dropped(log: &[String], field: &str) -> bool {
    log.iter()
        .any(|line| line.contains("dropped") && line.contains(field))
}

// --- Criterion: chat with tools, streamed and whole ---------------------------------------------

/// A streamed chat request with a tool reaches `InvokeModel` with response stream for the
/// configured model id, signed with the container credentials; the tool call Bedrock streams is
/// answered as chat tool-call deltas, and the effort Bedrock does not take is dropped by name.
#[test]
fn bedrock_a_streamed_chat_tool_turn_reaches_invoke_signed_and_answers_tool_call_deltas() {
    let deployment = Deployment::new("bedrock-chat-stream");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_tool_call()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let request = json!({
        "model": "haiku",
        "stream": true,
        "reasoning_effort": "high",
        "messages": [{"role": "user", "content": PROMPT_MARKER}],
        "tools": chat_tools(),
    });
    let answer = server.post("/v1/chat/completions", &request.to_string());
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("content-type: text/event-stream\r\n"),
        "{answer}"
    );
    assert!(answer.contains("\"tool_calls\""), "{answer}");
    assert!(answer.contains("\"name\":\"get_weather\""), "{answer}");
    assert!(answer.contains("Berlin"), "{answer}");
    assert!(
        answer.contains("\"finish_reason\":\"tool_calls\""),
        "{answer}"
    );
    assert!(answer.contains("data: [DONE]"), "{answer}");

    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let sent = &seen[0];
    assert!(
        sent.starts_with(&format!(
            "POST /model/{MODEL_ID}/invoke-with-response-stream HTTP/1.1\r\n"
        )),
        "{sent}"
    );
    assert_eq!(header(sent, "accept"), Some(EVENT_STREAM), "{sent}");
    assert_signed(sent);
    let sent = body(sent);
    assert_eq!(sent["anthropic_version"], "bedrock-2023-05-31", "{sent}");
    assert!(sent.get("model").is_none(), "{sent}");
    assert!(sent.get("stream").is_none(), "{sent}");
    assert!(sent.get("output_config").is_none(), "{sent}");
    assert_eq!(sent["tools"][0]["name"], "get_weather", "{sent}");
    assert_fetched(&credentials);
    assert!(dropped(&log, "reasoning_effort"), "{log:?}");
    assert_quiet(&log);
}

/// The same tool turn without `stream` is answered as one chat completion carrying the call.
#[test]
fn bedrock_a_whole_chat_tool_turn_is_answered_with_the_tool_call() {
    let deployment = Deployment::new("bedrock-chat-whole");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_tool_call()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let request = json!({
        "model": "haiku",
        "messages": [{"role": "user", "content": PROMPT_MARKER}],
        "tools": chat_tools(),
        "tool_choice": "auto",
    });
    let answer = server.post("/v1/chat/completions", &request.to_string());
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(
        answer.contains("content-type: application/json\r\n"),
        "{answer}"
    );
    let completion: Value = serde_json::from_str(answer.split_once("\r\n\r\n").unwrap().1).unwrap();
    let call = &completion["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "get_weather", "{completion}");
    let arguments: Value =
        serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({"city": "Berlin"}), "{completion}");
    assert_eq!(
        completion["choices"][0]["finish_reason"], "tool_calls",
        "{completion}"
    );
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_signed(&seen[0]);
    assert_quiet(&log);
}

/// The warm probe a client sends first, `max_tokens: 1`, reaches Bedrock with that bound and is
/// answered.
#[test]
fn bedrock_the_one_token_warm_probe_is_carried_and_answered() {
    let deployment = Deployment::new("bedrock-probe");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = server.post(
        "/v1/chat/completions",
        &json!({"model":"haiku","max_tokens":1,"messages":[{"role":"user","content":PROMPT_MARKER}]})
            .to_string(),
    );
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains(ANSWER_MARKER), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(body(&seen[0])["max_tokens"], 1, "{}", seen[0]);
    assert_quiet(&log);
}

// --- Criterion: Claude Code on the messages wire -----------------------------------------------

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

const CLAUDE_CODE_FIRST_TURN: &str =
    include_str!("../../llm-gateway/tests/fixtures/claude-code/1-tool-turn.request.txt");
const CLAUDE_CODE_TOOL_RESULT: &str =
    include_str!("../../llm-gateway/tests/fixtures/claude-code/2-tool-result.request.txt");

/// Posts a Claude Code fixture turn to a Bedrock alias `code` on `/v1/messages?beta=true`, and
/// returns the answer, the body Bedrock received and the log.
fn claude_code(name: &str, fixture: &str) -> (String, Value, Vec<String>) {
    let deployment = Deployment::new(name);
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_call("Read", "file_path")]);
    let config = deployment.config(&bedrock_table("code", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = server.post("/v1/messages?beta=true", &fixture_body(fixture));
    let log = server.log();
    let seen = bedrock.seen();
    assert!(!answer.contains("event: error"), "{answer}");
    assert_eq!(seen.len(), 1, "{answer} {seen:?}");
    assert_signed(&seen[0]);
    assert_fetched(&credentials);
    (answer, body(&seen[0]), log)
}

/// Claude Code's first turn reaches `InvokeModel` with its system blocks, `cache_control`,
/// adaptive thinking and tools; its effort, which the target does not take, is dropped and
/// logged by name; the tool call is answered as Messages events.
#[test]
fn bedrock_claude_codes_first_turn_reaches_invoke_and_is_answered_on_messages() {
    let (answer, sent, log) = claude_code("bedrock-claude-code-1", CLAUDE_CODE_FIRST_TURN);
    let fixture: Value = serde_json::from_str(&fixture_body(CLAUDE_CODE_FIRST_TURN)).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains("event: message_stop"), "{answer}");
    assert!(answer.contains("\"type\":\"tool_use\""), "{answer}");
    assert!(answer.contains("\"name\":\"Read\""), "{answer}");
    assert_eq!(sent["anthropic_version"], "bedrock-2023-05-31", "{sent}");
    assert_eq!(sent["max_tokens"], fixture["max_tokens"], "{sent}");
    assert_eq!(sent["thinking"], json!({"type": "adaptive"}), "{sent}");
    assert!(sent.get("output_config").is_none(), "{sent}");
    assert!(sent.get("model").is_none(), "{sent}");
    assert!(sent.to_string().contains("cache_control"), "{sent}");
    assert_eq!(
        sent["tools"].as_array().map(Vec::len),
        fixture["tools"].as_array().map(Vec::len),
        "{sent}"
    );
    assert!(dropped(&log, "output_config.effort"), "{log:?}");
    assert_quiet(&log);
}

/// Claude Code's second turn carries its signed thinking block and tool result back to
/// `InvokeModel` unchanged.
#[test]
fn bedrock_claude_codes_tool_result_turn_carries_its_signed_thinking_back() {
    let (answer, sent, log) = claude_code("bedrock-claude-code-2", CLAUDE_CODE_TOOL_RESULT);
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let text = sent.to_string();
    assert!(
        text.contains("\"signature\":\"3b1f0c9e8a7d4c2b9e6f5a4d3c2b1a09\""),
        "{text}"
    );
    assert!(text.contains("\"type\":\"tool_result\""), "{text}");
    assert!(text.contains("chatcmpl-tool-a1b2c3d4e5f60718"), "{text}");
    assert_quiet(&log);
}

// --- Criterion: Converse for a model that is not Anthropic's ------------------------------------

/// A model id that names no Anthropic model is sent to Converse by default; the effort and the
/// parallel tool call control, which Converse does not carry, are dropped and logged by name.
#[test]
fn bedrock_a_model_that_is_not_anthropics_reaches_converse_without_the_fields_it_refuses() {
    let deployment = Deployment::new("bedrock-converse");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![converse_text()]);
    let config = deployment.config(&bedrock_table(
        "nova",
        "eu.amazon.nova-fixture-v1:0",
        &bedrock,
        "",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let request = json!({
        "model": "nova",
        "stream": true,
        "reasoning_effort": "low",
        "parallel_tool_calls": false,
        "messages": [{"role": "user", "content": PROMPT_MARKER}],
        "tools": chat_tools(),
    });
    let answer = server.post("/v1/chat/completions", &request.to_string());
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    assert!(answer.contains(ANSWER_MARKER), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0]
            .starts_with("POST /model/eu.amazon.nova-fixture-v1%3A0/converse-stream HTTP/1.1\r\n"),
        "{}",
        seen[0]
    );
    assert_signed(&seen[0]);
    let sent = body(&seen[0]);
    assert_eq!(
        sent["toolConfig"]["tools"][0]["toolSpec"]["name"], "get_weather",
        "{sent}"
    );
    assert!(dropped(&log, "reasoning_effort"), "{log:?}");
    assert!(dropped(&log, "parallel_tool_calls"), "{log:?}");
    assert_quiet(&log);
}

/// A turn Converse's own encoder would refuse is refused `translation-unsupported` (400) at
/// preparation, never sent and never counted as an upstream failure: a conversation that opens
/// with an assistant message, and a follow-up carrying tool history but no `tools`.
#[test]
fn bedrock_a_turn_converse_cannot_carry_is_refused_before_anything_is_sent() {
    let deployment = Deployment::new("bedrock-converse-refusals");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![converse_text(), converse_text()]);
    let config = deployment.config(&bedrock_table(
        "nova",
        "eu.amazon.nova-fixture-v1:0",
        &bedrock,
        "",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let assistant_first = json!({
        "model": "nova",
        "messages": [
            {"role": "assistant", "content": "an earlier answer"},
            {"role": "user", "content": PROMPT_MARKER},
        ],
    });
    let tool_history_without_tools = json!({
        "model": "nova",
        "messages": [
            {"role": "user", "content": PROMPT_MARKER},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "call_1", "type": "function",
                "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}]},
            {"role": "tool", "tool_call_id": "call_1", "content": "sunny"},
        ],
    });
    for (case, request) in [
        ("assistant first", assistant_first),
        ("tool history without tools", tool_history_without_tools),
    ] {
        let answer = server.post("/v1/chat/completions", &request.to_string());
        assert!(
            answer.starts_with("HTTP/1.1 400 "),
            "{case}: refused at preparation, not as an upstream refusal: {answer}"
        );
        assert!(
            answer.contains("translation-unsupported"),
            "{case}: {answer}"
        );
    }
    let log = server.log();
    assert!(bedrock.seen().is_empty(), "{:?}", bedrock.seen());
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
    assert_quiet(&log);
}

/// A model that declares no `anthropic_beta` list carries no beta: Claude Code's
/// `anthropic-beta` header is not carried into the Invoke body as `anthropic_beta`, and the
/// header's name alone is logged as dropped, never its value.
#[test]
fn bedrock_an_anthropic_beta_header_is_dropped_by_name_and_never_carried() {
    let deployment = Deployment::new("bedrock-anthropic-beta");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let beta = "fixture-beta-2026-10-23";
    let request = json!({"model":"haiku","max_tokens":64,
        "messages":[{"role":"user","content":PROMPT_MARKER}]})
    .to_string();
    let answer = server.exchange(&format!(
        "POST /v1/messages?beta=true HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nanthropic-version: 2023-06-01\r\nanthropic-beta: {beta}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{request}",
        request.len()
    ));
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        body(&seen[0]).get("anthropic_beta").is_none(),
        "{}",
        seen[0]
    );
    assert!(!seen[0].contains(beta), "{}", seen[0]);
    assert!(dropped(&log, "anthropic-beta"), "{log:?}");
    for line in &log {
        assert!(!line.contains(beta), "the beta value was logged: {line}");
    }
    assert_quiet(&log);
}

/// Posts a messages request for `model` to `server` with `beta` as its one `anthropic-beta` line.
fn post_with_beta(server: &Server, model: &str, beta: &str) -> String {
    let request = json!({"model":model,"max_tokens":64,
        "messages":[{"role":"user","content":PROMPT_MARKER}]})
    .to_string();
    server.exchange(&format!(
        "POST /v1/messages?beta=true HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\nanthropic-version: 2023-06-01\r\nanthropic-beta: {beta}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{request}",
        request.len()
    ))
}

/// The log lines that say the beta `name` was dropped.
fn beta_dropped(log: &[String], name: &str) -> bool {
    log.iter().any(|line| {
        line.contains("dropped") && line.contains("anthropic-beta") && line.contains(name)
    })
}

/// An `InvokeModel` model's `anthropic_beta` list is what reaches Bedrock: the client's names on
/// it go into the Invoke body's `anthropic_beta` in the client's order, and each other name is
/// left out and logged by name as dropped.
#[test]
fn bedrock_allowlisted_beta_names_reach_the_invoke_body_in_the_clients_order() {
    let deployment = Deployment::new("bedrock-beta-allowlist");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table(
        "haiku",
        MODEL_ID,
        &bedrock,
        "anthropic_beta = [\"fixture-beta-b\", \"fixture-beta-a\", \"fixture-beta-unsent\"]\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = post_with_beta(
        &server,
        "haiku",
        "claude-code-fixture-1, fixture-beta-a,oauth-fixture-2 , fixture-beta-b",
    );
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_signed(&seen[0]);
    assert_eq!(
        body(&seen[0])["anthropic_beta"],
        json!(["fixture-beta-a", "fixture-beta-b"]),
        "{}",
        seen[0]
    );
    for name in [
        "claude-code-fixture-1",
        "oauth-fixture-2",
        "fixture-beta-unsent",
    ] {
        assert!(!seen[0].contains(name), "{name}: {}", seen[0]);
    }
    assert!(beta_dropped(&log, "claude-code-fixture-1"), "{log:?}");
    assert!(beta_dropped(&log, "oauth-fixture-2"), "{log:?}");
    assert!(!beta_dropped(&log, "fixture-beta-a"), "{log:?}");
    assert!(!beta_dropped(&log, "fixture-beta-b"), "{log:?}");
    assert_quiet(&log);
}

/// A client that names no beta on the list sends none: the body has no `anthropic_beta`.
#[test]
fn bedrock_a_client_naming_no_allowlisted_beta_sends_none() {
    let deployment = Deployment::new("bedrock-beta-none-listed");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table(
        "haiku",
        MODEL_ID,
        &bedrock,
        "anthropic_beta = [\"fixture-beta-a\"]\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = post_with_beta(&server, "haiku", "claude-code-fixture-1");
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        body(&seen[0]).get("anthropic_beta").is_none(),
        "{}",
        seen[0]
    );
    assert!(beta_dropped(&log, "claude-code-fixture-1"), "{log:?}");
}

/// A Converse model carries no beta name, whatever the client sends: the header is logged as
/// dropped and no name reaches Bedrock.
#[test]
fn bedrock_a_converse_model_never_carries_a_beta_name() {
    let deployment = Deployment::new("bedrock-beta-converse");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![converse_text()]);
    let config = deployment.config(&bedrock_table(
        "nova",
        "eu.amazon.nova-fixture-v1:0",
        &bedrock,
        "",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let beta = "fixture-beta-2026-10-23";
    let answer = post_with_beta(&server, "nova", beta);
    let log = server.log();
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0].contains("/converse-stream HTTP/1.1\r\n"),
        "{}",
        seen[0]
    );
    assert!(!seen[0].contains(beta), "{}", seen[0]);
    assert!(!seen[0].contains("anthropic_beta"), "{}", seen[0]);
    assert!(dropped(&log, "anthropic-beta"), "{log:?}");
    assert_quiet(&log);
}

/// An `anthropic-beta` header llm's decoder refuses (a name outside the rules, more names than a
/// turn carries) refuses the request as a bad body is refused, before Bedrock or the
/// credentials endpoint is asked.
#[test]
fn bedrock_a_malformed_beta_header_is_refused_before_anything_is_sent() {
    let deployment = Deployment::new("bedrock-beta-malformed");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table(
        "haiku",
        MODEL_ID,
        &bedrock,
        "anthropic_beta = [\"fixture-beta-a\"]\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let too_many = (0..=MAX_ANTHROPIC_BETAS)
        .map(|n| format!("fixture-beta-{n}"))
        .collect::<Vec<_>>()
        .join(",");
    for (case, beta) in [
        (
            "a name with a slash",
            "fixture-beta-a,fixture/beta".to_owned(),
        ),
        (
            "a name with a quote",
            "fixture-beta-a,\"fixture\"".to_owned(),
        ),
        ("more names than a turn carries", too_many),
    ] {
        let answer = post_with_beta(&server, "haiku", &beta);
        assert!(answer.starts_with("HTTP/1.1 400 "), "{case}: {answer}");
        assert!(answer.contains("translation-invalid"), "{case}: {answer}");
    }
    let log = server.log();
    assert!(bedrock.seen().is_empty(), "{:?}", bedrock.seen());
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
    assert_quiet(&log);
}

/// `api = "anthropic-invoke"` chooses `InvokeModel` for an id the default would send to Converse.
#[test]
fn bedrock_the_api_key_overrides_the_default_operation() {
    let deployment = Deployment::new("bedrock-api-key");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let profile = "arn:aws:bedrock:eu-central-1:000000000000:application-inference-profile/fixture";
    let config = deployment.config(&bedrock_table(
        "profile",
        profile,
        &bedrock,
        "api = \"anthropic-invoke\"\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = server.post(
        "/v1/chat/completions",
        &json!({"model":"profile","messages":[{"role":"user","content":PROMPT_MARKER}]})
            .to_string(),
    );
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(
        seen[0].contains("/invoke-with-response-stream HTTP/1.1\r\n"),
        "{}",
        seen[0]
    );
}

/// A declared `max_output_tokens` bounds the output a request naming none is given, and the
/// inventory reports it.
#[test]
fn bedrock_a_declared_output_limit_bounds_the_default_output() {
    let deployment = Deployment::new("bedrock-output-limit");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table(
        "haiku",
        MODEL_ID,
        &bedrock,
        "max_output_tokens = 2000\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let answer = server.post(
        "/v1/chat/completions",
        &json!({"model":"haiku","messages":[{"role":"user","content":PROMPT_MARKER}]}).to_string(),
    );
    let routes = server.get("/v1/routes");
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 OK\r\n"), "{answer}");
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(body(&seen[0])["max_tokens"], json!(2000), "{}", seen[0]);
    assert!(routes.contains("\"max_output_tokens\":2000"), "{routes}");
}

// --- Criterion: the inventory --------------------------------------------------------------------

/// The inventory lists a Bedrock model with provider `bedrock`, and no byte the process writes
/// holds its region, its model id, its endpoint or the credentials URI.
#[test]
fn bedrock_the_inventory_lists_the_target_without_its_region_model_id_or_endpoint() {
    let deployment = Deployment::new("bedrock-inventory");
    let credentials = Recorder::credentials(4);
    let bedrock = Recorder::start(EVENT_STREAM, Vec::new());
    let config = deployment.config(&bedrock_table(
        "haiku",
        MODEL_ID,
        &bedrock,
        "billing_kind = \"metered\"\n",
    ));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    let routes = server.get("/v1/routes");
    let models = server.get("/v1/models");
    let log = server.log();
    assert!(routes.starts_with("HTTP/1.1 200 "), "{routes}");
    assert!(
        routes.contains("\"target_id\":\"haiku.bedrock\""),
        "{routes}"
    );
    assert!(routes.contains("\"provider\":\"bedrock\""), "{routes}");
    assert!(models.contains("\"id\":\"haiku\""), "{models}");
    let mut written = routes;
    written.push_str(&models);
    for line in &log {
        written.push_str(line);
    }
    for withheld in [
        REGION,
        MODEL_ID,
        &bedrock.address.to_string(),
        &credentials.address.to_string(),
        CONTAINER_TOKEN,
    ] {
        assert!(
            !written.contains(withheld),
            "{withheld:?} was written: {written}"
        );
    }
}

// --- Criterion: what refuses the start -----------------------------------------------------------

/// A document whose Bedrock entry carries an endpoint key, or whose endpoint entry carries a
/// Bedrock key, or names a region, an api or a credential source Bedrock does not have, is
/// refused before anything is served.
#[test]
fn bedrock_a_document_mixing_or_misnaming_bedrock_keys_is_refused() {
    let deployment = Deployment::new("bedrock-document");
    let credentials = Recorder::credentials(0);
    let bedrock = Recorder::start(EVENT_STREAM, Vec::new());
    let environment = Environment::container(&credentials, &deployment.token);
    for (case, table) in [
        (
            "base_url on bedrock",
            bedrock_table("haiku", MODEL_ID, &bedrock, "base_url = \"http://127.0.0.1:9/v1\"\n"),
        ),
        (
            "wires on bedrock",
            bedrock_table("haiku", MODEL_ID, &bedrock, "wires = [\"chat\"]\n"),
        ),
        (
            "auth_kind on bedrock",
            bedrock_table("haiku", MODEL_ID, &bedrock, "auth_kind = \"anonymous\"\n"),
        ),
        (
            "region on an endpoint",
            "[hosted.local]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nauth_kind = \"anonymous\"\nregion = \"eu-central-1\"\n".to_owned(),
        ),
        (
            "an upper-case region",
            bedrock_table("haiku", MODEL_ID, &bedrock, "").replace(REGION, "EU_CENTRAL_1"),
        ),
        (
            "an empty model id",
            bedrock_table("haiku", MODEL_ID, &bedrock, "").replace(MODEL_ID, ""),
        ),
        (
            "an unknown api",
            bedrock_table("haiku", MODEL_ID, &bedrock, "api = \"invoke\"\n"),
        ),
        (
            "an unknown credential source",
            bedrock_table("haiku", MODEL_ID, &bedrock, "")
                .replace("credential_source = \"container\"", "credential_source = \"static\""),
        ),
        (
            "an endpoint off loopback over http",
            bedrock_table("haiku", MODEL_ID, &bedrock, "")
                .replace(&format!("http://{}", bedrock.address), "http://bedrock.example.test"),
        ),
        (
            "max_output_tokens on an endpoint",
            "[hosted.local]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nauth_kind = \"anonymous\"\nmax_output_tokens = 4096\n".to_owned(),
        ),
        (
            "a zero output limit",
            bedrock_table("haiku", MODEL_ID, &bedrock, "max_output_tokens = 0\n"),
        ),
        (
            "an output limit above the window",
            bedrock_table(
                "haiku",
                MODEL_ID,
                &bedrock,
                "context_window = 8192\nmax_output_tokens = 8193\n",
            ),
        ),
        (
            "converse with a reasoning effort",
            bedrock_table(
                "nova",
                "eu.amazon.nova-fixture-v1:0",
                &bedrock,
                "reasoning_effort = \"high\"\n",
            ),
        ),
        (
            "anthropic_beta on an endpoint",
            "[hosted.local]\nwires = [\"messages\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nauth_kind = \"anonymous\"\nanthropic_beta = [\"fixture-beta-a\"]\n".to_owned(),
        ),
        (
            "anthropic_beta on a converse model",
            bedrock_table(
                "nova",
                "eu.amazon.nova-fixture-v1:0",
                &bedrock,
                "anthropic_beta = [\"fixture-beta-a\"]\n",
            ),
        ),
        (
            "anthropic_beta on a model api names converse",
            bedrock_table(
                "haiku",
                MODEL_ID,
                &bedrock,
                "api = \"converse\"\nanthropic_beta = [\"fixture-beta-a\"]\n",
            ),
        ),
        (
            "a beta name outside the name rules",
            bedrock_table("haiku", MODEL_ID, &bedrock, "anthropic_beta = [\"fixture beta\"]\n"),
        ),
        (
            "an empty beta name",
            bedrock_table("haiku", MODEL_ID, &bedrock, "anthropic_beta = [\"\"]\n"),
        ),
        (
            "a beta name given twice",
            bedrock_table(
                "haiku",
                MODEL_ID,
                &bedrock,
                "anthropic_beta = [\"fixture-beta-a\", \"fixture-beta-a\"]\n",
            ),
        ),
        (
            "more beta names than a turn carries",
            bedrock_table(
                "haiku",
                MODEL_ID,
                &bedrock,
                &format!(
                    "anthropic_beta = [{}]\n",
                    (0..=MAX_ANTHROPIC_BETAS)
                        .map(|n| format!("\"fixture-beta-{n}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        ),
    ] {
        let config = deployment.config(&table);
        let stderr = assert_refused(&config, &environment, "config:value");
        assert!(!stderr.contains(CONTAINER_TOKEN), "{case}: {stderr}");
        if case.contains("beta") {
            assert!(stderr.contains(".anthropic_beta"), "{case}: {stderr}");
        }
    }
}

/// A Bedrock entry without the container credentials environment refuses the start, and so do
/// a credentials URI the client does not admit and a token file that cannot be trusted.
#[test]
fn bedrock_a_start_without_usable_container_credentials_is_refused() {
    let deployment = Deployment::new("bedrock-environment");
    let credentials = Recorder::credentials(0);
    let bedrock = Recorder::start(EVENT_STREAM, Vec::new());
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    assert_refused(&config, &Environment::none(), "container-credentials:unset");
    let only_uri = Environment {
        variables: vec![(
            URI_VARIABLE,
            format!("http://{}/v1/credentials", credentials.address),
        )],
    };
    assert_refused(&config, &only_uri, "container-credentials:unset");
    let remote = Environment {
        variables: vec![
            (
                URI_VARIABLE,
                "http://credentials.example.test/v1".to_owned(),
            ),
            (TOKEN_VARIABLE, deployment.token.display().to_string()),
        ],
    };
    let stderr = assert_refused(&config, &remote, "container-credentials:uri");
    assert!(!stderr.contains("credentials.example.test"), "{stderr}");
    let missing = Environment {
        variables: vec![
            (
                URI_VARIABLE,
                format!("http://{}/v1/credentials", credentials.address),
            ),
            (
                TOKEN_VARIABLE,
                deployment.fixture.path("absent").display().to_string(),
            ),
        ],
    };
    assert_refused(&config, &missing, "container-token:unreadable");
    let writable = deployment.fixture.write(
        "writable-token",
        format!("{CONTAINER_TOKEN}\n").as_bytes(),
        0o666,
    );
    let unsafe_mode = Environment::container(&credentials, &writable);
    assert_refused(&config, &unsafe_mode, "container-token:unsafe-mode");
    let empty = deployment.fixture.write("empty-token", b"\n", 0o600);
    assert_refused(
        &config,
        &Environment::container(&credentials, &empty),
        "container-token:not-a-token",
    );
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
}

/// A document without a Bedrock entry needs no container credentials environment.
#[test]
fn bedrock_a_document_without_a_bedrock_entry_ignores_the_environment() {
    let deployment = Deployment::new("bedrock-not-declared");
    let config = deployment.config(
        "[hosted.local]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nauth_kind = \"anonymous\"\n",
    );
    let server = Server::start(&config, &Environment::none());
    drop(server);
}

// --- Criterion: the token as a Kubernetes projected volume mounts it ----------------------------

/// `seconds` after the Unix epoch as an RFC 3339 UTC instant.
fn rfc3339(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap();
    let rest = seconds % 86_400;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// A container credentials endpoint whose keys expire two minutes from now, inside llm-bedrock's
/// refresh window, so every request fetches them again and presents the token file's token.
fn credentials_expiring_soon(count: usize) -> Recorder {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let answer = json!({
        "AccessKeyId": ACCESS_KEY_ID,
        "SecretAccessKey": SECRET_ACCESS_KEY,
        "Token": SESSION_TOKEN,
        "Expiration": rfc3339(now + 120),
    })
    .to_string()
    .into_bytes();
    Recorder::start("application/json", vec![answer; count])
}

const PROJECTED_TOKEN: &str = "eks-pod-identity-token";
const ROTATED_TOKEN: &str = "fake-rotated-container-authorization-token-9876543210";

/// A projected volume as the kubelet writes one: `<volume>/eks-pod-identity-token` links to
/// `..data/eks-pod-identity-token`, `..data` links to a timestamped directory holding the token
/// at mode 0644. The layout is from the AWS and Kubernetes documentation, unverified here
/// against a cluster.
struct ProjectedVolume {
    dir: PathBuf,
}

impl ProjectedVolume {
    fn new(fixture: &Fixture, token: &str) -> Self {
        let dir = fixture.path("pod-identity");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let volume = Self { dir };
        volume.generation("..2026_10_23_00_00_00.000000001", token);
        std::os::unix::fs::symlink("..2026_10_23_00_00_00.000000001", volume.dir.join("..data"))
            .unwrap();
        std::os::unix::fs::symlink(
            format!("..data/{PROJECTED_TOKEN}"),
            volume.dir.join(PROJECTED_TOKEN),
        )
        .unwrap();
        volume
    }

    fn generation(&self, name: &str, token: &str) {
        let generation = self.dir.join(name);
        std::fs::create_dir(&generation).unwrap();
        std::fs::set_permissions(&generation, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = generation.join(PROJECTED_TOKEN);
        std::fs::write(&file, token.as_bytes()).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    /// Rotates as the kubelet does: a new timestamped directory, `..data` swapped to it by a
    /// rename, the old directory removed.
    fn rotate(&self, token: &str) {
        self.generation("..2026_10_23_01_00_00.000000002", token);
        let staged = self.dir.join("..data_tmp");
        std::os::unix::fs::symlink("..2026_10_23_01_00_00.000000002", &staged).unwrap();
        std::fs::rename(&staged, self.dir.join("..data")).unwrap();
        std::fs::remove_dir_all(self.dir.join("..2026_10_23_00_00_00.000000001")).unwrap();
    }

    fn token(&self) -> PathBuf {
        self.dir.join(PROJECTED_TOKEN)
    }
}

/// The authorization each credentials fetch presented, in order.
fn presented(credentials: &Recorder) -> Vec<String> {
    credentials
        .seen()
        .iter()
        .map(|request| {
            header(request, "authorization")
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// A token file mounted as a Kubernetes projected volume, behind the `..data` symlinks at mode
/// 0644, is accepted at start and presented to the credentials endpoint; once the kubelet
/// rotates it, the next request presents the new token without a restart.
#[test]
fn bedrock_a_projected_token_volume_is_followed_and_its_rotated_token_is_used() {
    let deployment = Deployment::new("bedrock-projected");
    let volume = ProjectedVolume::new(&deployment.fixture, CONTAINER_TOKEN);
    let credentials = credentials_expiring_soon(4);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text(), invoke_text()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &volume.token()),
    );
    let request =
        json!({"model":"haiku","messages":[{"role":"user","content":PROMPT_MARKER}]}).to_string();
    let first = server.post("/v1/chat/completions", &request);
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    assert_eq!(presented(&credentials), vec![CONTAINER_TOKEN.to_owned()]);
    volume.rotate(&format!("{ROTATED_TOKEN}\n"));
    let second = server.post("/v1/chat/completions", &request);
    let log = server.log();
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert_eq!(
        presented(&credentials),
        vec![CONTAINER_TOKEN.to_owned(), ROTATED_TOKEN.to_owned()]
    );
    let seen = bedrock.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_signed(&seen[1]);
    assert_quiet(&log);
    for line in &log {
        assert!(
            !line.contains(ROTATED_TOKEN),
            "the token was logged: {line}"
        );
    }
}

/// A linked token file is held to the rules of the file the links resolve to: a link to a
/// world-writable file is `unsafe-mode` and a link to nothing is `unreadable`. Every other
/// trusted file still refuses a final symlink.
#[test]
fn bedrock_a_linked_token_is_held_to_the_rules_of_the_file_it_resolves_to() {
    let deployment = Deployment::new("bedrock-linked-token");
    let credentials = Recorder::credentials(0);
    let bedrock = Recorder::start(EVENT_STREAM, Vec::new());
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let writable = deployment.fixture.write(
        "writable-token",
        format!("{CONTAINER_TOKEN}\n").as_bytes(),
        0o666,
    );
    let to_writable = deployment.fixture.path("linked-writable-token");
    std::os::unix::fs::symlink(&writable, &to_writable).unwrap();
    assert_refused(
        &config,
        &Environment::container(&credentials, &to_writable),
        "container-token:unsafe-mode",
    );
    let dangling = deployment.fixture.path("linked-absent-token");
    std::os::unix::fs::symlink(deployment.fixture.path("absent"), &dangling).unwrap();
    assert_refused(
        &config,
        &Environment::container(&credentials, &dangling),
        "container-token:unreadable",
    );
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
}

/// A token file that becomes untrusted after the start makes the next fetch `unavailable`; the
/// gateway warns with the refusal's rule and the file's path, and never with the token.
#[test]
fn bedrock_a_token_file_refused_at_a_fetch_is_warned_by_its_rule_and_path_without_the_token() {
    let deployment = Deployment::new("bedrock-token-refused-at-fetch");
    let credentials = Recorder::credentials(1);
    let bedrock = Recorder::start(EVENT_STREAM, vec![invoke_text()]);
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let server = Server::start(
        &config,
        &Environment::container(&credentials, &deployment.token),
    );
    std::fs::set_permissions(&deployment.token, std::fs::Permissions::from_mode(0o666)).unwrap();
    let request =
        json!({"model":"haiku","messages":[{"role":"user","content":PROMPT_MARKER}]}).to_string();
    let answer = server.post("/v1/chat/completions", &request);
    let log = server.log();
    assert!(!answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
    assert!(bedrock.seen().is_empty(), "{:?}", bedrock.seen());
    let path = deployment.token.display().to_string();
    assert!(
        log.iter().any(|line| line.contains("WARN")
            && line.contains("container-token:unsafe-mode")
            && line.contains(&path)),
        "{log:?}"
    );
    assert_quiet(&log);
}

/// A token link that resolves to a file outside the directory the token path names is refused
/// `container-token:symlink`, naming the file it resolves to; a projected volume's links stay
/// inside that directory.
#[test]
fn bedrock_a_token_link_resolving_outside_its_directory_is_refused() {
    let deployment = Deployment::new("bedrock-token-outside");
    let credentials = Recorder::credentials(0);
    let bedrock = Recorder::start(EVENT_STREAM, Vec::new());
    let config = deployment.config(&bedrock_table("haiku", MODEL_ID, &bedrock, ""));
    let mount = deployment.fixture.path("mount");
    std::fs::create_dir(&mount).unwrap();
    std::fs::set_permissions(&mount, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = mount.join("token");
    std::os::unix::fs::symlink(&deployment.token, &link).unwrap();
    let stderr = assert_refused(
        &config,
        &Environment::container(&credentials, &link),
        "container-token:symlink",
    );
    let resolved = std::fs::canonicalize(&deployment.token).unwrap();
    assert!(stderr.contains(&resolved.display().to_string()), "{stderr}");
    assert!(!stderr.contains(CONTAINER_TOKEN), "{stderr}");
    assert!(credentials.seen().is_empty(), "{:?}", credentials.seen());
}
