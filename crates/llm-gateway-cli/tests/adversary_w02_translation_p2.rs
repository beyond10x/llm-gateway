//! Adversary, story:gateway-translation pass 2: cases the correction round does not state.
//!
//! Each drives the binary's `LlmTranslator` through the port as the relay does (`prepare`, then
//! `run` against an acquired target and the client's writer), against a loopback upstream.
//! Nothing leaves the machine and no credential is real.

use llm_gateway::{
    AnswerFraming, Label, RefusalCode, RelayModel, RelayStream, RelayTarget, ToolCalling,
    TranslatedAnswer, TranslationFailure, TranslationRequest, Translator, Wire,
};
use llm_gateway_cli::LlmTranslator;
use std::{
    fmt::Write as _,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(30);

const MESSAGE_START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n";

const TEXT_OPENING: &str = concat!(
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
);

const TEXT_CLOSING: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const TOOL_CLOSING: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const TOOLS: &str = "\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"write\",\"description\":\"Write a file\",\"parameters\":{\"type\":\"object\"}}}]";

struct Loopback(String);

impl RelayTarget for Loopback {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::other("the translator opens its own connection"))
    }
}

#[derive(Default)]
struct Recorded {
    head: Option<(u16, String, AnswerFraming)>,
    body: Vec<u8>,
}

impl TranslatedAnswer for Recorded {
    fn head(&mut self, status: u16, content_type: &str, framing: AnswerFraming) -> bool {
        if self.head.is_some() {
            return false;
        }
        self.head = Some((status, content_type.to_owned(), framing));
        true
    }

    fn body(&mut self, bytes: &[u8]) -> bool {
        if self.head.is_none() {
            return false;
        }
        self.body.extend_from_slice(bytes);
        true
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

/// Answers one connection with `body` as an event stream framed by `content-length`; the thread
/// returns the request body it read.
fn upstream(body: String) -> (SocketAddr, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection.set_read_timeout(Some(WAIT)).unwrap();
        let request = String::from_utf8_lossy(&read_request(&mut connection)).into_owned();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        drop(connection.write_all(head.as_bytes()));
        drop(connection.write_all(body.as_bytes()));
        drop(connection.flush());
        drop(connection.shutdown(Shutdown::Both));
        request
            .split_once("\r\n\r\n")
            .map_or_else(String::new, |(_, body)| body.to_owned())
    });
    (address, thread)
}

fn model(window: Option<u64>) -> RelayModel {
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed);
    match window {
        Some(window) => model.with_max_model_len(window),
        None => model,
    }
}

struct Ran {
    result: Result<(), TranslationFailure>,
    answer: Recorded,
    upstream_body: String,
}

/// Prepares the chat `request` for `model`'s messages upstream, runs it against an upstream
/// answering `body`.
fn run(model: &RelayModel, request: &str, body: String) -> Ran {
    let prepared = LlmTranslator::new()
        .prepare(&TranslationRequest {
            ingress: Wire::Chat,
            upstream: Wire::Messages,
            model,
            body: request.as_bytes(),
            anthropic_beta: false,
        })
        .unwrap_or_else(|code| panic!("prepare refused {code:?}"));
    let (address, thread) = upstream(body);
    let mut answer = Recorded::default();
    let result = prepared.run(&Loopback(address.to_string()), &mut answer);
    Ran {
        result,
        answer,
        upstream_body: thread.join().unwrap(),
    }
}

/// A messages answer holding one `write` tool call whose arguments, `{"content":"aaa…"}`, are
/// `bytes` long, streamed in deltas of at most 8192 bytes.
fn tool_answer(bytes: usize) -> String {
    let opening = "{\"content\":\"";
    let closing = "\"}";
    let mut fragments = vec![opening.to_owned()];
    let mut left = bytes - opening.len() - closing.len();
    while left > 0 {
        let piece = left.min(8192);
        fragments.push("a".repeat(piece));
        left -= piece;
    }
    fragments.push(closing.to_owned());
    let mut body = String::from(MESSAGE_START);
    body.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"write\",\"input\":{}}}\n\n");
    for fragment in fragments {
        let fragment = serde_json::to_string(&fragment).unwrap();
        write!(
            body,
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{fragment}}}}}\n\n"
        )
        .unwrap();
    }
    body.push_str(TOOL_CLOSING);
    body
}

/// `docs/gateway.md` "Bounds of the translated path" publishes the bounds a translated answer is
/// held to: 65536 events, 33554432 bytes, a 1048576-byte line, and an outcome of 4096 items or
/// 16 MiB; "an answer at a bound is written whole". A tool call of 70000 argument bytes, in
/// 15 events of under 9 KiB each, passes none of them, and the byte relay relays it. llm 0.7.0's
/// messages stream decoder refuses streamed tool arguments past 65536 bytes
/// (`MAX_TOOL_ARGUMENT_BYTES`, `crates/llm-messages/src/decode.rs` `input_json_delta`), so the
/// answer ends with `translation-too-large`'s terminal error, whose message names three bounds
/// the answer did not pass. Reached by any agent's file-writing tool call of more than 64 KiB
/// on a translated route. A 60000-byte call, under that bound, is written whole first, so the
/// fixture is not what fails.
///
/// Re-pinned in correction round 2. Asserted first: the 70000-byte call is written whole.
/// Decided instead: the bound is published (`translated-tool-argument-bytes`, 65536) and the
/// terminal message names the bound family, so the 70000-byte call ends with
/// `translation-too-large`'s terminal error while the 60000-byte one is written whole.
#[test]
fn adv_p2_a_tool_call_past_64_kib_of_arguments_ends_with_the_bounds_terminal_error() {
    let request = format!(
        "{{\"model\":\"code\",\"stream\":true,\"messages\":[{{\"role\":\"user\",\"content\":\"Write it\"}}],{TOOLS}}}"
    );
    let ended = |bytes: usize| {
        let ran = run(&model(None), &request, tool_answer(bytes));
        assert_eq!(ran.result, Ok(()));
        let written = String::from_utf8_lossy(&ran.answer.body).into_owned();
        let tail = written[written.len().saturating_sub(400)..].to_owned();
        (written.ends_with("data: [DONE]\n\n"), tail)
    };
    let (whole, tail) = ended(60_000);
    assert!(
        whole,
        "the 60000-byte control is not written whole: ...{tail}"
    );
    let (whole, tail) = ended(70_000);
    assert!(
        !whole && tail.ends_with(&bound_terminal_error()),
        "a 70000-byte tool call does not end with the bound's terminal error: ...{tail}"
    );
}

/// The chat chunk a translated answer past a bound ends with.
fn bound_terminal_error() -> String {
    format!(
        "data: {{\"error\":{{\"code\":\"translation-too-large\",\"message\":\"{}\"}}}}\n\n",
        RefusalCode::TranslationTooLarge.reason()
    )
}

/// The decision on pass 1's finding 4: a request naming no output limit gets
/// `min(16384, window / 2)` on a messages upstream. A model that declares no window has the
/// translator's `DEFAULT_CONTEXT_WINDOW`, 200000, as its window and output limit, so it is asked
/// for 16384. The unit sends the whole 200000 instead (and pins that in
/// `translation_bound_a_messages_upstream_is_asked_for_half_the_window_at_most_16384`), the
/// overrun the decision closes. Reached by every `[hosted.*]` messages endpoint declared
/// without `context_window` (optional in the TOML), from a chat or responses client that names
/// no limit.
#[test]
fn adv_p2_a_messages_upstream_of_a_model_without_a_window_is_asked_for_at_most_16384() {
    let request = "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    let ran = run(
        &model(None),
        request,
        format!("{MESSAGE_START}{TEXT_OPENING}{TEXT_CLOSING}"),
    );
    assert_eq!(ran.result, Ok(()));
    let sent: serde_json::Value = serde_json::from_str(&ran.upstream_body).unwrap();
    assert_eq!(
        sent["max_tokens"].as_u64(),
        Some(16_384),
        "the upstream is asked for {} with no window declared",
        sent["max_tokens"]
    );
}

/// A well-formed `200` answer whose model called a tool the request did not publish is the
/// model's output failing llm's check (`TurnOutcome::validate_for`, `protocol`), not a target
/// that was unreachable, timed out, or cut or garbled its answer, the cases
/// `RefusalCode::UpstreamFailed` and `docs/gateway.md` name. Answered before any output (a
/// non-streamed request), it is `UpstreamFailed`, so the relay invalidates a healthy target and
/// tells the client "the next request asks for a replacement": a pod is replaced because its
/// model hallucinated a tool name. Reached by a small open model on a vLLM pod with parsed tool
/// calling, from any non-streamed chat or messages client offering tools.
#[test]
fn adv_p2_a_model_that_calls_an_unpublished_tool_does_not_fail_its_target() {
    let request = format!(
        "{{\"model\":\"code\",\"messages\":[{{\"role\":\"user\",\"content\":\"Write it\"}}],{TOOLS}}}"
    );
    let calling = |name: &str| {
        let mut body = String::from(MESSAGE_START);
        write!(body, "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"{name}\",\"input\":{{}}}}}}\n\n").unwrap();
        body.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n");
        body.push_str(TOOL_CLOSING);
        body
    };
    // The control: the same answer calling the published tool is answered.
    let control = run(&model(None), &request, calling("write"));
    assert_eq!(control.result, Ok(()));
    assert_eq!(control.answer.head.map(|(status, ..)| status), Some(200));
    let ran = run(&model(None), &request, calling("delete"));
    assert!(ran.answer.head.is_none(), "nothing reaches the client");
    assert_ne!(
        ran.result,
        Err(TranslationFailure::UpstreamFailed),
        "a model's unpublished tool call reports its target failed, and the relay invalidates it"
    );
}

/// The published line bound is 1048576 bytes; an event whose data spans two lines of about
/// 600000 bytes each (JSON whitespace split across `data:` lines, which SSE joins with `\n`)
/// passes no published bound, yet llm's per-event data bound (`MAX_EVENT_BYTES` applied to the
/// joined data, `crates/llm-http/src/sse.rs`) refuses it, and the answer ends with
/// `translation-too-large`. The table names the constant as the bytes "of one line". No
/// upstream was found that splits an event's data across lines, so this is a document finding.
///
/// Re-pinned in correction round 2. Asserted first: the event of two 600000-byte lines is
/// written whole. Decided instead: the bound is documented as an event's joined data
/// (`translated-event-bytes`), so an event whose joined data passes 1048576 ends with the bound's
/// terminal error, and one of two 400000-byte lines, under it, is written whole.
#[test]
fn adv_p2_an_event_whose_joined_data_passes_the_event_bound_ends_with_its_terminal_error() {
    let request = "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    let ended = |padding: usize| {
        let spaces = " ".repeat(padding);
        let split = format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,{spaces}\ndata: {spaces}\"delta\":{{\"type\":\"text_delta\",\"text\":\"lo\"}}}}\n\n"
        );
        let ran = run(
            &model(None),
            request,
            format!("{MESSAGE_START}{TEXT_OPENING}{split}{TEXT_CLOSING}"),
        );
        assert_eq!(ran.result, Ok(()));
        let written = String::from_utf8_lossy(&ran.answer.body).into_owned();
        let tail = written[written.len().saturating_sub(400)..].to_owned();
        (
            written.ends_with("data: [DONE]\n\n") && written.contains("\"lo\""),
            tail,
        )
    };
    let (whole, tail) = ended(400_000);
    assert!(
        whole,
        "an event under the bound is not written whole: ...{tail}"
    );
    let (whole, tail) = ended(600_000);
    assert!(
        !whole && tail.ends_with(&bound_terminal_error()),
        "an event past the bound does not end with its terminal error: ...{tail}"
    );
}
