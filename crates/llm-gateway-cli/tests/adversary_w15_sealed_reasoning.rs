//! Adversarial cases for story:cross-wire-reasoning-state: forged, re-encoded, truncated,
//! padded, oversized and cross-route reasoning seals replayed to the shipped binary, and a
//! same-wire route that must relay a seal-shaped item byte for byte.
//!
//! Every upstream is a loopback fixture recording what it receives; nothing leaves the machine
//! and no credential is real.

mod support;

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, hosted_document};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
const SEAL_PREFIX: &str = "llmgw-seal-1.";
const CODEX_FIRST_TURN: &str = include_str!("fixtures/codex/first-turn.json");
const CODEX_SECOND_TURN: &str = include_str!("fixtures/codex/second-turn.json");

const MESSAGES_ANSWER: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

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

const CHAT_REASONING_CALL: &str = concat!(
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"Read the file.\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_capture_1\",\"type\":\"function\",\"function\":{\"name\":\"exec_command\",\"arguments\":\"{\\\"cmd\\\": \\\"cat a.txt\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-up\",\"model\":\"served\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":9,\"total_tokens\":21}}\n\n",
    "data: [DONE]\n\n",
);

/// A minimal responses upstream answer, for the same-wire route.
const RESPONSES_ANSWER: &str = concat!(
    "event: response.created\ndata: {\"type\":\"response.created\",\"sequence_number\":0,\"response\":{\"id\":\"resp_up\",\"object\":\"response\",\"created_at\":1,\"status\":\"in_progress\",\"model\":\"served\",\"output\":[]}}\n\n",
    "event: response.completed\ndata: {\"type\":\"response.completed\",\"sequence_number\":1,\"response\":{\"id\":\"resp_up\",\"object\":\"response\",\"created_at\":1,\"status\":\"completed\",\"model\":\"served\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
);

struct Upstream {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Upstream {
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

    /// What it recorded so far, without waiting for its script to end.
    fn so_far(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|raw| String::from_utf8_lossy(raw).into_owned())
            .collect()
    }

    fn seen(mut self) -> Vec<String> {
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.so_far()
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

fn hosted_model(alias: &str, wire: &str, upstream: &Upstream) -> String {
    format!(
        "[hosted.{alias}]\nwires = [\"{wire}\"]\nbase_url = \"http://{}/v1\"\nupstream_model = \"served\"\nauth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n",
        upstream.address
    )
}

fn first_turn(model: &str) -> String {
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_FIRST_TURN).unwrap();
    codex["model"] = serde_json::json!(model);
    codex.to_string()
}

fn sealed_item(answer: &str) -> serde_json::Value {
    answer
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .find(|event| {
            event["type"] == "response.output_item.done" && event["item"]["type"] == "reasoning"
        })
        .map_or_else(
            || panic!("no finished reasoning item reached the client: {answer}"),
            |event| event["item"].clone(),
        )
}

/// Codex's recorded second turn to `model`, its reasoning item's `encrypted_content` set to
/// `content`.
fn second_turn(model: &str, content: &serde_json::Value) -> String {
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_SECOND_TURN).unwrap();
    codex["model"] = serde_json::json!(model);
    let replayed = codex["input"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["type"] == "reasoning")
        .unwrap();
    replayed["encrypted_content"] = content.clone();
    codex.to_string()
}

fn refused_opaque_state(answer: &str) -> bool {
    answer.starts_with("HTTP/1.1 400 ") && answer.contains("\"code\":\"translation-opaque-state\"")
}

/// Every re-encoding, truncation, padding, splice, oversize and cross-route variant of a seal the
/// gateway made is `translation-opaque-state` before any target is asked for, none of them takes
/// the gateway down, and the genuine seal still binds afterwards.
#[test]
fn adversary_w15_every_variant_of_a_seal_is_refused_and_the_genuine_one_still_binds() {
    let fixture = Fixture::new("adversary-w15-seal-variants");
    let code = Upstream::script(vec![MESSAGES_THINKING_CALL, MESSAGES_ANSWER]);
    let chat = Upstream::script(vec![CHAT_REASONING_CALL]);
    let server = Server::start(&document(
        &fixture,
        &format!(
            "{}{}",
            hosted_model("code", "messages", &code),
            hosted_model("chatty", "chat", &chat)
        ),
    ));
    let sealed =
        sealed_item(&server.post("/v1/responses", &first_turn("code")))["encrypted_content"]
            .as_str()
            .unwrap()
            .to_owned();
    let null_sealed =
        sealed_item(&server.post("/v1/responses", &first_turn("chatty")))["encrypted_content"]
            .as_str()
            .unwrap()
            .to_owned();
    let variants = seal_variants(&sealed, &null_sealed);
    let mut failures = Vec::new();
    for (name, content) in &variants {
        let answer = server.post("/v1/responses", &second_turn("code", content));
        if !refused_opaque_state(&answer) {
            failures.push(format!(
                "{name}: {}",
                answer.lines().next().unwrap_or("<no answer>")
            ));
        }
    }
    // The genuine seal on the chat route is not the chat route's.
    let crossed = server.post(
        "/v1/responses",
        &second_turn("chatty", &serde_json::json!(sealed)),
    );
    if !refused_opaque_state(&crossed) {
        failures.push(format!(
            "messages seal on the chat route: {}",
            crossed.lines().next().unwrap_or("")
        ));
    }
    assert_eq!(
        code.so_far().len(),
        1,
        "a refused variant reached the upstream"
    );
    let bound = server.post(
        "/v1/responses",
        &second_turn("code", &serde_json::json!(sealed)),
    );
    assert!(
        bound.starts_with("HTTP/1.1 200 OK\r\n"),
        "the genuine seal after the variants: {bound}"
    );
    drop(server);
    assert!(
        failures.is_empty(),
        "not refused translation-opaque-state: {failures:#?}"
    );
    assert_eq!(code.seen().len(), 2);
    assert_eq!(chat.seen().len(), 1);
}

/// Every re-encoding, truncation, padding, splice, oversize and cross-route variant of `sealed`,
/// the messages route's seal, with `null_sealed`, the chat route's seal over `null`.
fn seal_variants(sealed: &str, null_sealed: &str) -> Vec<(&'static str, serde_json::Value)> {
    assert!(sealed.starts_with(SEAL_PREFIX), "{sealed}");
    assert!(null_sealed.starts_with(SEAL_PREFIX), "{null_sealed}");
    let (state, tag) = sealed[SEAL_PREFIX.len()..].split_once('.').unwrap();
    let (null_state, null_tag) = null_sealed[SEAL_PREFIX.len()..].split_once('.').unwrap();
    let standard = sealed.replace('-', "+").replace('_', "/");

    let mut variants: Vec<(&str, serde_json::Value)> = vec![
        ("padded", serde_json::json!(format!("{sealed}="))),
        ("padded twice", serde_json::json!(format!("{sealed}=="))),
        ("leading space", serde_json::json!(format!(" {sealed}"))),
        ("trailing space", serde_json::json!(format!("{sealed} "))),
        ("trailing newline", serde_json::json!(format!("{sealed}\n"))),
        (
            "upper-case prefix",
            serde_json::json!(format!("LLMGW-SEAL-1.{state}.{tag}")),
        ),
        ("no prefix", serde_json::json!(format!("{state}.{tag}"))),
        (
            "other version",
            serde_json::json!(format!("llmgw-seal-2.{state}.{tag}")),
        ),
        (
            "double dot",
            serde_json::json!(format!("{SEAL_PREFIX}{state}..{tag}")),
        ),
        (
            "tag twice",
            serde_json::json!(format!("{SEAL_PREFIX}{state}.{tag}.{tag}")),
        ),
        (
            "tag cut by one",
            serde_json::json!(format!("{SEAL_PREFIX}{state}.{}", &tag[..tag.len() - 1])),
        ),
        (
            "tag empty",
            serde_json::json!(format!("{SEAL_PREFIX}{state}.")),
        ),
        (
            "state empty",
            serde_json::json!(format!("{SEAL_PREFIX}.{tag}")),
        ),
        ("prefix only", serde_json::json!(SEAL_PREFIX)),
        (
            "prefix and dot",
            serde_json::json!(format!("{SEAL_PREFIX}.")),
        ),
        ("empty string", serde_json::json!("")),
        ("null", serde_json::Value::Null),
        ("number", serde_json::json!(1)),
        ("array", serde_json::json!([sealed])),
        ("object", serde_json::json!({"seal": sealed})),
        (
            "null state under its tag",
            serde_json::json!(format!("{SEAL_PREFIX}{null_state}.{tag}")),
        ),
        (
            "this state under the null tag",
            serde_json::json!(format!("{SEAL_PREFIX}{state}.{null_tag}")),
        ),
        ("the chat route's null seal", serde_json::json!(null_sealed)),
        (
            "oversized state",
            serde_json::json!(format!("{SEAL_PREFIX}{}{state}.{tag}", "A".repeat(1 << 20))),
        ),
        (
            "oversized tag",
            serde_json::json!(format!("{SEAL_PREFIX}{state}.{tag}{}", "A".repeat(1 << 20))),
        ),
    ];
    if standard != sealed {
        variants.push(("standard alphabet", serde_json::json!(standard)));
    }
    variants
}

/// A same-wire route (a responses client to a responses upstream) is relayed byte for byte: a
/// reasoning item whose `encrypted_content` looks like a gateway seal reaches the upstream with
/// that `encrypted_content` unchanged and is not refused.
#[test]
fn adversary_w15_a_same_wire_route_relays_a_seal_shaped_item_unchanged() {
    let fixture = Fixture::new("adversary-w15-same-wire");
    let upstream = Upstream::script(vec![RESPONSES_ANSWER]);
    let server = Server::start(&document(
        &fixture,
        &hosted_model("code", "responses", &upstream),
    ));
    let forged = format!("{SEAL_PREFIX}bnVsbA.AAAA");
    let body = second_turn("code", &serde_json::json!(forged));
    let answer = server.post("/v1/responses", &body);
    drop(server);
    assert!(!refused_opaque_state(&answer), "{answer}");
    let seen = upstream.seen();
    assert_eq!(seen.len(), 1, "{answer}");
    let sent: serde_json::Value =
        serde_json::from_str(seen[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    let relayed = sent["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == "reasoning")
        .unwrap_or_else(|| panic!("the reasoning item did not reach the upstream: {sent}"));
    assert_eq!(
        relayed["encrypted_content"],
        serde_json::json!(forged),
        "{sent}"
    );
}

/// A messages upstream that thinks, signs, then emits a `redacted_thinking` block, then calls
/// `exec_command`.
const MESSAGES_TWO_BLOCKS_CALL: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Read the file.\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig-upstream-1\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"redacted_thinking\",\"data\":\"redacted-upstream-2\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_capture_1\",\"name\":\"exec_command\",\"input\":{}}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\": \\\"cat a.txt\\\"}\"}}\n\n",
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":9}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// An answer with two reasoning blocks (signed `thinking`, then `redacted_thinking`) hands the
/// client two sealed items, each finished once and in the upstream's order; Codex replaying
/// every finished item in the order it was finished reaches the upstream with both blocks, in
/// their place, before the call.
#[test]
fn adversary_w15_two_reasoning_blocks_are_sealed_and_bound_in_their_order() {
    let fixture = Fixture::new("adversary-w15-two-blocks");
    let upstream = Upstream::script(vec![MESSAGES_TWO_BLOCKS_CALL, MESSAGES_ANSWER]);
    let server = Server::start(&document(
        &fixture,
        &hosted_model("code", "messages", &upstream),
    ));
    let first = server.post("/v1/responses", &first_turn("code"));
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    // What Codex keeps: every finished output item, in the order it was finished.
    let finished: Vec<serde_json::Value> = first
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str::<serde_json::Value>(data).ok())
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["item"].clone())
        .collect();
    let reasoning: Vec<&serde_json::Value> = finished
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect();
    assert_eq!(reasoning.len(), 2, "{first}");
    let ids: Vec<&serde_json::Value> = reasoning.iter().map(|item| &item["id"]).collect();
    assert_ne!(ids[0], ids[1], "{first}");
    let mut codex: serde_json::Value = serde_json::from_str(CODEX_SECOND_TURN).unwrap();
    codex["model"] = serde_json::json!("code");
    let input = codex["input"].as_array_mut().unwrap();
    let at = input
        .iter()
        .position(|entry| entry["type"] == "reasoning")
        .unwrap();
    input.remove(at);
    for (offset, item) in reasoning.iter().enumerate() {
        let mut replayed = (*item).clone();
        replayed.as_object_mut().unwrap().remove("status");
        input.insert(at + offset, replayed);
    }
    let second = server.post("/v1/responses", &codex.to_string());
    drop(server);
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    let seen = upstream.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let sent: serde_json::Value =
        serde_json::from_str(seen[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    let assistant = sent["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap_or_else(|| panic!("no assistant turn reached the upstream: {sent}"));
    let blocks = assistant["content"].as_array().unwrap();
    assert_eq!(
        blocks[0],
        serde_json::json!({"type": "thinking", "thinking": "Read the file.", "signature": "sig-upstream-1"}),
        "{sent}"
    );
    assert_eq!(
        blocks[1],
        serde_json::json!({"type": "redacted_thinking", "data": "redacted-upstream-2"}),
        "{sent}"
    );
    assert_eq!(blocks[2]["type"], "tool_use", "{sent}");
}
