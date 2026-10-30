//! Adversary cases, wave 2026-10-30-w06 pass 1, story:client-profiles-translated, through the
//! shipped binary.
//!
//! The story's acceptance: "`GET /` with a Codex or Claude Code `user-agent` names only models
//! that client's requests reach without a `translation-*` refusal". `docs/gateway.md` "The public
//! routes" says the same: "`GET /` thus names only models a client's requests reach without a
//! `translation-*` refusal". Admission prepares one recorded first turn per client. The cases
//! below send the other Claude Code 2.1.293 requests this repository records
//! (`crates/llm-gateway/tests/fixtures/claude-code/`) to a translated model `GET /` profiles
//! Claude Code for, and drive the translation guide's pasted `GET /` answers against the binary.
//!
//! Every upstream is a loopback fixture; nothing leaves the machine.

mod support;

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, hosted_document};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);

const CLAUDE_CODE_FIRST_TURN: &str =
    include_str!("../src/admission/claude-code/1-tool-turn.request.txt");
const CLAUDE_CODE_SECOND_TURN: &str =
    include_str!("../../llm-gateway/tests/fixtures/claude-code/2-tool-result.request.txt");
const CLAUDE_CODE_STRUCTURED_OUTPUT: &str =
    include_str!("../../llm-gateway/tests/fixtures/claude-code/3-structured-output.request.txt");
const TRANSLATION_GUIDE: &str =
    include_str!("../../../website/docs/guides/translate-between-wires.md");

const CLAUDE_CODE_AGENT: &str = "claude-cli/2.1.293 (external, cli)";

/// A chat upstream that shows its reasoning as `reasoning_content` and calls Claude Code's
/// `Read`, as the first answer of `2-tool-result.request.txt` did.
const CHAT_REASONING_READ: &str = concat!(
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"The user wants the file. I'll read it.\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_read_1\",\"type\":\"function\",\"function\":{\"name\":\"Read\",\"arguments\":\"{\\\"file_path\\\": \\\"/tmp/a.txt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":9,\"total_tokens\":21}}\n\n",
    "data: [DONE]\n\n",
);

/// A chat upstream answering `Hello` with 12 + 2 tokens.
const CHAT_ANSWER: &str = concat!(
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":2,\"total_tokens\":14}}\n\n",
    "data: [DONE]\n\n",
);

/// A chat upstream's whole answer to a request that is not streamed: the title as JSON text.
const CHAT_TITLE: &str = "{\"id\":\"chatcmpl-up\",\"object\":\"chat.completion\",\"model\":\"served\",\"choices\":[{\"index\":0,\"message\":{\"role\":\"assistant\",\"content\":\"{\\\"title\\\":\\\"Reading a.txt\\\"}\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":6,\"total_tokens\":18}}";

// --- A loopback upstream -----------------------------------------------------------------------

/// Answers each connection with the next of its scripted answers. Its thread is never joined: a
/// refused request never connects, and nothing waits for it.
struct Upstream {
    address: SocketAddr,
}

impl Upstream {
    fn script(answers: Vec<(&'static str, &'static str)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for (content_type, answer) in answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                read_request(&mut connection);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    answer.len()
                );
                drop(connection.write_all(head.as_bytes()));
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self { address }
    }
}

/// Reads one request: its head, then a `content-length` body.
fn read_request(connection: &mut TcpStream) {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16_384];
    let end = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return,
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
}

// --- The shipped binary ------------------------------------------------------------------------

struct Server {
    child: Child,
    address: SocketAddr,
}

impl Server {
    fn start(config: &Path) -> Self {
        Self::start_with(config, &[])
    }

    /// The binary with the container-credential variables removed, then `environment` set.
    fn start_with(config: &Path, environment: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"));
        command
            .args(["--config", config.to_str().unwrap()])
            .env_remove("AWS_CONTAINER_CREDENTIALS_FULL_URI")
            .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE")
            .env_remove("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI")
            .env_remove("AWS_CONTAINER_AUTHORIZATION_TOKEN")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (name, value) in environment {
            command.env(name, value);
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
        Self { child, address }
    }

    /// The whole answer to `request`, sent as it is.
    fn exchange(&self, request: &str) -> String {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// `GET /` without a credential, with `host` and `user_agent`.
    fn root(&self, host: &str, user_agent: &str) -> String {
        self.exchange(&format!(
            "GET / HTTP/1.1\r\nhost: {host}\r\nuser-agent: {user_agent}\r\n\r\n"
        ))
    }

    /// Claude Code's request as the authored `fixture` writes its head (request line and header
    /// lines, `?beta=true` and `anthropic-beta` included), with the owner's credential and `body`.
    fn claude_code(&self, fixture: &str, body: &str) -> String {
        let text = without_provenance(fixture);
        let (head, _) = text.split_once("\n\n").unwrap();
        let mut lines = head.lines();
        let request_line = lines.next().unwrap();
        let mut request =
            format!("{request_line}\r\nhost: gateway\r\nuser-agent: {CLAUDE_CODE_AGENT}\r\n");
        for line in lines {
            request.push_str(line);
            request.push_str("\r\n");
        }
        let length = body.len();
        request.push_str("authorization: Bearer ");
        request.push_str(OWNER_SECRET);
        request.push_str("\r\ncontent-length: ");
        request.push_str(&length.to_string());
        request.push_str("\r\n\r\n");
        request.push_str(body);
        self.exchange(&request)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// An authored fixture without its `#` provenance lines.
fn without_provenance(fixture: &str) -> String {
    fixture
        .lines()
        .skip_while(|line| line.starts_with('#'))
        .fold(String::new(), |mut text, line| {
            text.push_str(line);
            text.push('\n');
            text
        })
}

/// The fixture's JSON body, naming `model`.
fn body_naming(fixture: &str, model: &str) -> Value {
    let text = without_provenance(fixture);
    let (_, body) = text.split_once("\n\n").unwrap();
    let mut body: Value = serde_json::from_str(body).unwrap();
    body["model"] = json!(model);
    body
}

/// A deployment serving one hosted model `alias` from a chat endpoint at `upstream`, tool calling
/// parsed and no capability declared: `GET /` profiles Claude Code for it through translation
/// (`translation_get_root_profiles_codex_and_claude_code_where_their_turns_translate`).
fn translated_chat_model(fixture: &Fixture, alias: &str, upstream: SocketAddr) -> PathBuf {
    let secret = fixture.owner_secret();
    let table = format!(
        "[hosted.{alias}]\nwires = [\"chat\"]\nbase_url = \"http://{upstream}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n"
    );
    fixture.config(&hosted_document("127.0.0.1:0", &secret, &table))
}

/// The payload of every `data:` line of an event stream.
fn events(answer: &str) -> Vec<Value> {
    answer
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect()
}

/// The assistant content a Messages client assembles from the event stream `answer`: each
/// block as `content_block_start` opened it, its text, thinking and signature deltas appended,
/// and a `tool_use` block's input parsed from its `input_json_delta` parts. This is how
/// `2-tool-result.request.txt` says Claude Code replays its previous answer ("the assistant
/// turn as the stream assembled it").
fn assembled(answer: &str) -> Vec<Value> {
    let mut blocks: Vec<Value> = Vec::new();
    let mut inputs: Vec<String> = Vec::new();
    for event in events(answer) {
        match event["type"].as_str() {
            Some("content_block_start") => {
                blocks.push(event["content_block"].clone());
                inputs.push(String::new());
            }
            Some("content_block_delta") => {
                let index = usize::try_from(event["index"].as_u64().unwrap()).unwrap();
                let delta = &event["delta"];
                let (key, part) = match delta["type"].as_str() {
                    Some("text_delta") => ("text", &delta["text"]),
                    Some("thinking_delta") => ("thinking", &delta["thinking"]),
                    Some("signature_delta") => ("signature", &delta["signature"]),
                    Some("input_json_delta") => {
                        inputs[index].push_str(delta["partial_json"].as_str().unwrap());
                        continue;
                    }
                    _ => continue,
                };
                let block = &mut blocks[index];
                let joined = format!(
                    "{}{}",
                    block[key].as_str().unwrap_or_default(),
                    part.as_str().unwrap_or_default()
                );
                block[key] = json!(joined);
            }
            _ => {}
        }
    }
    for (block, input) in blocks.iter_mut().zip(&inputs) {
        if block["type"] == "tool_use" && !input.is_empty() {
            block["input"] = serde_json::from_str(input).unwrap();
        }
    }
    blocks
}

/// Whether `answer` is a `translation-*` refusal.
fn refused_translation(answer: &str) -> bool {
    answer.contains("\"code\":\"translation-")
}

/// Claude Code's second turn (`2-tool-result.request.txt`), replaying the answer `GET /`'s
/// translated model gave its first turn as the stream assembled it, reaches the model without a
/// `translation-*` refusal. A translated route refuses a turn replaying a `thinking` block
/// (`docs/gateway.md` "Reasoning state on a translated route", "What is not sealed"); this holds
/// because the gateway shows a Messages client no block for a chat upstream's
/// `reasoning_content`, so the replay carries none. Green when written (pass 1); it guards the
/// two-turn path no case of the unit drives.
#[test]
fn adversary_w06_claude_codes_second_turn_reaches_the_translated_model_get_root_names() {
    let fixture = Fixture::new("adv-w06-second-turn");
    let upstream = Upstream::script(vec![
        ("text/event-stream", CHAT_REASONING_READ),
        ("text/event-stream", CHAT_ANSWER),
    ]);
    let server = Server::start(&translated_chat_model(
        &fixture,
        "chat-up",
        upstream.address,
    ));
    let root = server.root("gateway", CLAUDE_CODE_AGENT);
    assert!(
        root.contains("export ANTHROPIC_MODEL='chat-up'\n"),
        "precondition, GET / profiles Claude Code for the translated model: {root}"
    );
    let first = server.claude_code(
        CLAUDE_CODE_FIRST_TURN,
        &body_naming(CLAUDE_CODE_FIRST_TURN, "chat-up").to_string(),
    );
    assert!(
        first.starts_with("HTTP/1.1 200 "),
        "precondition, the first turn is translated: {first}"
    );
    let blocks = assembled(&first);
    let call = blocks
        .iter()
        .find(|block| block["type"] == "tool_use")
        .unwrap_or_else(|| panic!("precondition, the first answer calls Read: {first}"))
        .clone();
    let mut second = body_naming(CLAUDE_CODE_SECOND_TURN, "chat-up");
    second["messages"][1]["content"] = Value::Array(blocks.clone());
    second["messages"][2]["content"][0]["tool_use_id"] = call["id"].clone();
    let answer = server.claude_code(CLAUDE_CODE_SECOND_TURN, &second.to_string());
    assert!(
        !refused_translation(&answer),
        "GET / named `chat-up` for Claude Code, and its second turn replaying {blocks:?} was refused:\n{answer}"
    );
}

/// Claude Code's structured-output side request (`3-structured-output.request.txt`,
/// `output_config.format`, which Claude Code 2.1.293 sends unless
/// `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1` is set) names the profile's alias, since the profile
/// sets every model setting, `ANTHROPIC_DEFAULT_HAIKU_MODEL` included, to it. On a translated
/// model `GET /` profiles Claude Code for, it is refused `translation-unsupported`
/// (`translation_fields_llm_still_refuses_are_refused_by_name`). Pass 1 asserted the request
/// reaches the model. DECIDED 2026-10-30 (story:client-profiles-translated, correction round 1):
/// the profile that names a model reached only by translation exports
/// `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS`, so Claude Code does not send the request; a profile
/// naming only a model that declares `messages` does not, and keeps structured outputs.
#[test]
fn adversary_w06_claude_codes_structured_output_request_reaches_the_translated_model_get_root_names()
 {
    const DISABLE: &str = "export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'\n";
    let fixture = Fixture::new("adv-w06-structured-output");
    let upstream = Upstream::script(vec![("application/json", CHAT_TITLE)]);
    let server = Server::start(&translated_chat_model(
        &fixture,
        "chat-up",
        upstream.address,
    ));
    let root = server.root("gateway", CLAUDE_CODE_AGENT);
    assert!(
        root.contains("export ANTHROPIC_DEFAULT_HAIKU_MODEL='chat-up'\n"),
        "precondition, GET / profiles Claude Code for the translated model: {root}"
    );
    assert!(
        root.contains(DISABLE),
        "story:client-profiles-translated, DECIDED 2026-10-30: the profile naming the translated `chat-up` exports CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1:\n{root}"
    );
    let answer = server.claude_code(
        CLAUDE_CODE_STRUCTURED_OUTPUT,
        &body_naming(CLAUDE_CODE_STRUCTURED_OUTPUT, "chat-up").to_string(),
    );
    assert!(
        refused_translation(&answer),
        "story:client-profiles-translated, DECIDED 2026-10-30: the request the variable stops is still refused on a translated path, so the variable is needed:\n{answer}"
    );
    let native = Fixture::new("adv-w06-structured-output-native");
    let secret = native.owner_secret();
    let table = "[hosted.msg-up]\nwires = [\"messages\"]\nbase_url = \"http://127.0.0.1:9/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n";
    let server = Server::start(&native.config(&hosted_document("127.0.0.1:0", &secret, table)));
    let root = server.root("gateway", CLAUDE_CODE_AGENT);
    assert!(
        root.contains("export ANTHROPIC_MODEL='msg-up'\n"),
        "precondition, GET / profiles Claude Code for the native messages model: {root}"
    );
    assert!(
        !root.contains("CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS"),
        "story:client-profiles-translated, DECIDED 2026-10-30: a profile naming only a native messages model keeps structured outputs:\n{root}"
    );
}

/// The unit hand-wrote the translation guide's pasted `GET /` answers for its routes document
/// (Claude Code handed `codex` only, Codex handed `claude` and `codex`). Driven against the
/// binary with the same routes, on loopback endpoints, each answer appears in the pasted block
/// byte for byte. Green when written (pass 1).
#[test]
fn adversary_w06_the_translation_guides_pasted_get_root_answers_are_the_binarys() {
    let fixture = Fixture::new("adv-w06-guide");
    let routes = "format: llm.routes/1
endpoints:
  anthropic:
    base_url: http://127.0.0.1:9/v1
    protocol: messages
    auth: {kind: anonymous}
  openrouter:
    base_url: http://127.0.0.1:9/api/v1
    protocol: chat-completions
    auth: {kind: anonymous}
routes:
  codex: anthropic/claude-sonnet-4-5
  claude: openrouter/anthropic/claude-sonnet-4.5
";
    let routes_file = fixture.write("routes.yaml", routes.as_bytes(), 0o600);
    let secret = fixture.owner_secret();
    let config = fixture.config(&format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\nroutes = \"{}\"\n",
        secret.display(),
        routes_file.display()
    ));
    let server = Server::start(&config);
    let body = |answer: String| answer.split_once("\r\n\r\n").unwrap().1.to_owned();
    let claude = body(server.root("127.0.0.1:8080", "claude-cli/2.1.0 (external, cli)"));
    let codex = body(server.root("127.0.0.1:8080", "codex_cli_rs/0.50.0"));
    let from = TRANSLATION_GUIDE
        .find("-A \"codex_cli_rs/0.50.0\"")
        .expect("the guide pastes GET / for Codex");
    let rest = &TRANSLATION_GUIDE[from..];
    let open = rest.find("```text\n").unwrap() + "```text\n".len();
    let close = rest[open..].find("```").unwrap();
    let pasted = &rest[open..open + close];
    assert!(
        pasted.contains(&claude),
        "Claude Code:\n{claude}\npasted:\n{pasted}"
    );
    assert!(
        pasted.contains(&codex),
        "Codex:\n{codex}\npasted:\n{pasted}"
    );
}

// --- Bedrock models (translated-only) ----------------------------------------------------------

/// A deployment serving the Bedrock models `tables`, with a container token file and a
/// credentials URI on loopback; `GET /` fetches no credential, so nothing listens there.
fn bedrock_deployment(fixture: &Fixture, tables: &str) -> (PathBuf, Vec<(&'static str, String)>) {
    let secret = fixture.owner_secret();
    let token = fixture.write(
        "container-token",
        b"fake-container-token-0123456789\n",
        0o600,
    );
    let config = fixture.config(&hosted_document("127.0.0.1:0", &secret, tables));
    let environment = vec![
        (
            "AWS_CONTAINER_CREDENTIALS_FULL_URI",
            "http://127.0.0.1:9/v1/credentials".to_owned(),
        ),
        (
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            token.display().to_string(),
        ),
    ];
    (config, environment)
}

/// A Bedrock table `alias` for an Anthropic model id, as the Bedrock guide declares it, with
/// `lines` after its keys.
fn bedrock_table(alias: &str, lines: &str) -> String {
    format!(
        "[hosted.{alias}]\nkind = \"bedrock\"\nregion = \"eu-central-1\"\nmodel_id = \"eu.anthropic.claude-haiku-5-5\"\ncredential_source = \"container\"\nendpoint = \"http://127.0.0.1:9\"\ncontext_window = 200000\n{lines}\n"
    )
}

/// The Bedrock guide's model (`website/docs/guides/serve-a-bedrock-model.md`, "Declare the
/// model"), which Claude Code's `claude -p` turn reaches through `InvokeModel`, is profiled for
/// Claude Code and for Codex: llm's Bedrock check admits both recorded first turns. Before the
/// unit every client was; the unit keeps the two whose turns pass.
#[test]
fn adversary_w06_get_root_profiles_claude_code_and_codex_for_the_bedrock_guides_model() {
    let fixture = Fixture::new("adv-w06-bedrock-guide");
    let (config, environment) = bedrock_deployment(&fixture, &bedrock_table("haiku", ""));
    let server = Server::start_with(&config, &environment);
    let claude = server.root("gateway", CLAUDE_CODE_AGENT);
    let codex = server.root("gateway", "codex_cli_rs/0.160.0");
    assert!(
        claude.contains("export ANTHROPIC_MODEL='haiku'\n"),
        "{claude}"
    );
    assert!(codex.contains("model = \"haiku\"\n"), "{codex}");
}

/// A Bedrock model whose `max_output_tokens` (a documented key, "1 to the window") is under the
/// 32000 Claude Code's recorded first turn asks for is, rightly, not profiled for Claude Code.
/// The answer then says no model declares the messages wire with tool calling parsed, while the
/// model declares exactly that (`tool_calling` defaults to `parsed` on a Bedrock model, and a
/// Bedrock model declares `messages`). The gateway-crate case
/// `adversary_w06_the_no_profile_answer_does_not_deny_a_wire_a_model_declares` holds the same
/// text; this one shows the binary reaches it from a valid deployment document.
#[test]
fn adversary_w06_a_bedrock_model_claude_code_is_not_admitted_to_is_not_said_to_lack_the_messages_wire()
 {
    let fixture = Fixture::new("adv-w06-bedrock-short-output");
    let (config, environment) = bedrock_deployment(
        &fixture,
        &bedrock_table("haiku", "max_output_tokens = 8192\n"),
    );
    let server = Server::start_with(&config, &environment);
    let claude = server.root("gateway", CLAUDE_CODE_AGENT);
    assert!(claude.starts_with("HTTP/1.1 200 "), "{claude}");
    assert!(
        !claude.contains("ANTHROPIC_MODEL='haiku'"),
        "precondition, Claude Code is not profiled for a model refusing its 32000 output tokens: {claude}"
    );
    assert!(
        !claude.contains("none declares the messages wire"),
        "`haiku` declares the messages wire with tool calling parsed, yet the answer says none does:\n{claude}"
    );
}

// --- Pass 2, on the fix 45272e7 ----------------------------------------------------------------

/// A messages upstream answering `Hello` with 12 + 2 tokens, as an event stream.
const MESSAGES_ANSWER: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// `POST path` with the owner's credential and `body`.
fn owner_post(server: &Server, path: &str, body: &str) -> String {
    server.exchange(&format!(
        "POST {path} HTTP/1.1\r\nhost: gateway\r\ncontent-type: application/json\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    ))
}

/// The Claude Code block of `alias` in the answer `claude`: from `# model <alias>` to the next
/// block or the end.
fn claude_block<'a>(claude: &'a str, alias: &str) -> &'a str {
    let marker = format!("\n# model {alias}\n");
    let start = claude
        .find(&marker)
        .unwrap_or_else(|| panic!("no block for {alias}: {claude}"))
        + 1;
    let rest = &claude[start..];
    let end = rest[1..]
        .find("\n# model ")
        .map_or(rest.len(), |end| end + 2);
    &rest[..end]
}

/// The fix rewords the no-profile line to "none with tool calling parsed takes its requests on
/// the <wire> wire, relayed or translated" and renders it for Loom too (`listing.rs` `plain`,
/// `none_for("Loom", Wire::Chat)`). Loom has no recorded turn, so the binary never admits it to
/// a translated model and never asks whether its requests translate; yet the gateway translates
/// a Loom chat request to a model that declares only `messages`
/// (`website/docs/guides/translate-between-wires.md`: "A request inside the subset, from any
/// client, is translated and sent"). The old text, "none declares the chat wire", was true here;
/// the new one says the chat request just served is not taken translated.
#[test]
fn adversary_w06_p2_the_loom_no_profile_answer_does_not_deny_the_chat_translation_it_serves() {
    let fixture = Fixture::new("adv-w06-p2-loom-none");
    let upstream = Upstream::script(vec![("text/event-stream", MESSAGES_ANSWER)]);
    let secret = fixture.owner_secret();
    let table = format!(
        "[hosted.msg-up]\nwires = [\"messages\"]\nbase_url = \"http://{}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n",
        upstream.address
    );
    let server = Server::start(&fixture.config(&hosted_document("127.0.0.1:0", &secret, &table)));
    let plain = server.root("gateway", "curl/8.5.0");
    assert!(
        plain.contains("# No model of this gateway serves Loom"),
        "precondition, no Loom profile for a messages-only model: {plain}"
    );
    let chat = json!({
        "model": "msg-up",
        "messages": [{"role": "user", "content": "Hi"}],
        "tools": [{"type": "function", "function": {"name": "read", "description": "Read a file", "parameters": {"type": "object"}}}],
        "stream": true
    })
    .to_string();
    let answer = owner_post(&server, "/v1/chat/completions", &chat);
    assert!(
        answer.starts_with("HTTP/1.1 200 ") && answer.contains("Hello"),
        "precondition, the gateway translates a Loom chat request with a tool to `msg-up`:\n{answer}"
    );
    assert!(
        !plain.contains("takes its requests on the chat wire, relayed or translated"),
        "GET / says no model with tool calling parsed takes Loom's requests translated, and the gateway just translated one to `msg-up` (tool calling parsed):\n{plain}"
    );
}

/// The new comment line carries an apostrophe ("Claude Code's"). A model block pasted into an
/// interactive zsh, whose default leaves `INTERACTIVE_COMMENTS` off, reads `#` as a command and
/// the apostrophe as an opening quote; the quote then swallows the next line, the very
/// `export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'` the fix adds, and the shell waits at
/// `quote>`. Measured with zsh 5.9: `zsh -f -i` fed the block ends `zsh: unmatched '`, the
/// variable unset; the same block without the two new lines sets every variable
/// (`command not found: #` for `# model <alias>` only). Before the fix no comment line of a
/// model block held a quote character.
#[test]
fn adversary_w06_p2_a_model_blocks_comment_lines_hold_no_quote_a_pasting_shell_would_open() {
    let fixture = Fixture::new("adv-w06-p2-quote");
    let upstream = Upstream::script(Vec::new());
    let server = Server::start(&translated_chat_model(
        &fixture,
        "chat-up",
        upstream.address,
    ));
    let claude = server.root("gateway", CLAUDE_CODE_AGENT);
    let block = claude_block(&claude, "chat-up");
    assert!(
        block.contains("export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'\n"),
        "precondition, the translated block exports the variable: {block}"
    );
    for line in block.lines().filter(|line| line.starts_with('#')) {
        assert!(
            !line.contains(['\'', '"', '`']),
            "a comment line of a model block opens a quote in an interactive zsh and swallows the export after it: {line:?}\n{block}"
        );
    }
}

/// Green guard: in one answer, the variable follows each model's route. `a-native` (messages
/// only) and `b-both` (chat and messages) are relayed, `c-chat` (chat only) is reached
/// translated; only `c-chat`'s block exports it, and the whole answer runs under POSIX `sh`.
#[test]
fn adversary_w06_p2_the_variable_follows_the_route_of_each_block_in_one_answer() {
    const DISABLE: &str = "export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'\n";
    let fixture = Fixture::new("adv-w06-p2-several");
    let secret = fixture.owner_secret();
    let mut tables = String::new();
    for (alias, wires) in [
        ("a-native", "\"messages\""),
        ("b-both", "\"chat\", \"messages\""),
        ("c-chat", "\"chat\""),
    ] {
        std::fmt::Write::write_fmt(
            &mut tables,
            format_args!(
                "[hosted.{alias}]\nwires = [{wires}]\nbase_url = \"http://127.0.0.1:9/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n\n"
            ),
        )
        .unwrap();
    }
    let server = Server::start(&fixture.config(&hosted_document("127.0.0.1:0", &secret, &tables)));
    let answer = server.root("gateway", CLAUDE_CODE_AGENT);
    let (_, claude) = answer.split_once("\r\n\r\n").unwrap();
    assert!(
        !claude_block(claude, "a-native").contains("DISABLE_STRUCTURED"),
        "{claude}"
    );
    assert!(
        !claude_block(claude, "b-both").contains("DISABLE_STRUCTURED"),
        "{claude}"
    );
    assert!(
        claude_block(claude, "c-chat").ends_with(DISABLE),
        "{claude}"
    );
    assert_eq!(claude.matches("DISABLE_STRUCTURED").count(), 1, "{claude}");
    let script = format!(
        "{claude}printf '%s|%s' \"$ANTHROPIC_MODEL\" \"$CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS\"\n"
    );
    let output = Command::new("sh").args(["-c", &script]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "c-chat|1",
        "{output:?}"
    );
}

/// Green guard: a Bedrock model (translated-only) gets the variable, and Claude Code's
/// structured-output request to it is refused `translation-unsupported` before any credential is
/// fetched, so the claim "Bedrock's included" holds through the binary.
#[test]
fn adversary_w06_p2_a_bedrock_block_exports_the_variable_its_structured_output_needs() {
    let fixture = Fixture::new("adv-w06-p2-bedrock");
    let (config, environment) = bedrock_deployment(&fixture, &bedrock_table("haiku", ""));
    let server = Server::start_with(&config, &environment);
    let claude = server.root("gateway", CLAUDE_CODE_AGENT);
    assert!(
        claude_block(&claude, "haiku")
            .ends_with("export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'\n"),
        "{claude}"
    );
    let answer = server.claude_code(
        CLAUDE_CODE_STRUCTURED_OUTPUT,
        &body_naming(CLAUDE_CODE_STRUCTURED_OUTPUT, "haiku").to_string(),
    );
    assert!(
        answer.contains("\"code\":\"translation-unsupported\""),
        "{answer}"
    );
}
