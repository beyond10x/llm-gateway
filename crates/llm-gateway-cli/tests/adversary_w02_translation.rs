//! Adversary, story:gateway-translation pass 1: cases the unit's suite does not state.
//!
//! Each drives the binary's `LlmTranslator` through the port exactly as the relay does
//! (`prepare`, then `run` against an acquired target and the client's writer), against a
//! loopback fixture upstream. Nothing leaves the machine and no credential is real.

use llm_gateway::{
    AnswerFraming, Label, RefusalCode, RelayModel, RelayStream, RelayTarget, ToolCalling,
    TranslatedAnswer, TranslationFailure, TranslationRequest, Translator, Wire,
};
use llm_gateway_cli::LlmTranslator;
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(20);

/// A messages upstream's opening events: the message, then one text block with `Hel`.
const MESSAGES_OPENING: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
);

/// A messages upstream's closing events.
const MESSAGES_CLOSING: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// A loopback target: the translator opens its own connection to the authority.
struct Loopback(String);

impl RelayTarget for Loopback {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::other("the translator opens its own connection"))
    }
}

/// The client's side, recorded.
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

/// Reads one request (head, then a `content-length` body) and returns it.
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

/// An upstream that answers one connection with `raw` (head and body as given) after `delay`,
/// then closes; the thread returns the request it read.
fn upstream(raw: Vec<u8>, delay: Duration) -> (SocketAddr, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let Ok((mut connection, _)) = listener.accept() else {
            return Vec::new();
        };
        connection.set_read_timeout(Some(WAIT)).unwrap();
        let request = read_request(&mut connection);
        thread::sleep(delay);
        drop(connection.write_all(&raw));
        drop(connection.flush());
        drop(connection.shutdown(Shutdown::Both));
        request
    });
    (address, thread)
}

fn model() -> RelayModel {
    RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed)
}

/// What one translation came to: its result, the client's side, and the request the upstream
/// read.
struct Ran {
    result: Result<(), TranslationFailure>,
    answer: Recorded,
    request: String,
}

/// Prepares `body` on the chat wire for `model`'s messages upstream with `translator`, runs it
/// against a loopback upstream answering `raw` after `delay`.
fn translate_with(
    translator: &LlmTranslator,
    model: &RelayModel,
    body: &str,
    raw: Vec<u8>,
    delay: Duration,
) -> Ran {
    let prepared = translator
        .prepare(&TranslationRequest {
            ingress: Wire::Chat,
            upstream: Wire::Messages,
            model,
            body: body.as_bytes(),
            anthropic_beta: &[],
        })
        .unwrap_or_else(|code| panic!("prepare refused {code:?}"));
    let (address, thread) = upstream(raw, delay);
    let mut answer = Recorded::default();
    let ran = prepared.run(&Loopback(address.to_string()), &mut answer);
    let request = String::from_utf8_lossy(&thread.join().unwrap()).into_owned();
    Ran {
        result: ran,
        answer,
        request,
    }
}

/// [`translate_with`] on a translator of its own, for [`model`], with no delay.
fn translate(body: &str, raw: Vec<u8>) -> (Result<(), TranslationFailure>, Recorded) {
    let ran = translate_with(&LlmTranslator::new(), &model(), body, raw, Duration::ZERO);
    (ran.result, ran.answer)
}

/// A whole messages answer `Hello`, framed by `content-length`.
fn messages_answer() -> Vec<u8> {
    let answer = format!("{MESSAGES_OPENING}{MESSAGES_CLOSING}");
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        answer.len()
    )
    .into_bytes();
    raw.extend_from_slice(answer.as_bytes());
    raw
}

/// A head declaring more body than the upstream then sends: the connection drops mid-answer.
fn cut_off(body: &str) -> Vec<u8> {
    let mut raw = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 65536\r\nconnection: close\r\n\r\n".to_vec();
    raw.extend_from_slice(body.as_bytes());
    raw
}

/// A target that answers its head and then drops the connection, before anything reached the
/// client, has failed; it has not refused. `RefusalCode::UpstreamRefused`'s own documentation
/// names `upstream-refused` "a 4xx, an authentication or a rate-limit refusal", and
/// `docs/gateway.md` "The answer" lists the same (and "an answer that is not an event stream");
/// `RefusalCode::UpstreamFailed` is the code of a target that "closed without an answer". So the
/// translation reports `UpstreamFailed`, the relay invalidates the target and the client reads
/// `upstream-failed`. Reached by a pod that dies mid-generation (preempted, OOM, restarted) on a
/// non-streamed request, whose answer the gateway writes only at the outcome, or before its first
/// event on a streamed one.
///
/// Re-pinned in correction round 2, which revises the decision of round 1: an abrupt close (an
/// end before the declared length, or inside a chunk) is still `upstream-failed`, but a stream
/// that ends cleanly before its terminal event is an answer that broke its wire, llm's
/// `protocol`, and is `upstream-refused`, invalidating nothing. The chunk and clean-end subcases
/// are added.
#[test]
fn adv_translation_a_target_that_drops_after_its_head_has_failed_not_refused() {
    let streamed = "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    let whole = "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    let failed = Err(TranslationFailure::UpstreamFailed);
    let refused = Err(TranslationFailure::Refused(RefusalCode::UpstreamRefused));
    let inside_a_chunk = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{MESSAGES_OPENING}",
        MESSAGES_OPENING.len() + 1000
    )
    .into_bytes();
    let clean_end = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{MESSAGES_OPENING}",
        MESSAGES_OPENING.len()
    )
    .into_bytes();
    let cases = [
        (
            "streamed, closed before any event",
            streamed,
            cut_off(""),
            failed,
        ),
        (
            "non-streamed, closed mid-answer",
            whole,
            cut_off(MESSAGES_OPENING),
            failed,
        ),
        (
            "non-streamed, closed inside a chunk",
            whole,
            inside_a_chunk,
            failed,
        ),
        (
            "non-streamed, ended cleanly before its terminal event",
            whole,
            clean_end,
            refused,
        ),
    ];
    let mut wrong = Vec::new();
    for (case, body, raw, expected) in cases {
        let (ran, answer) = translate(body, raw);
        assert!(answer.head.is_none(), "{case}: nothing reaches the client");
        if ran != expected {
            wrong.push(format!("{case}: {ran:?}, expected {expected:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "an abrupt close must be upstream-failed and a clean early end upstream-refused: {wrong:#?}"
    );
}

/// llm's chat ingress promises "Every field outside the published subset is refused", and the
/// story's design 5 that unsupported fields are "refused before dispatch, never dropped". Fields
/// below the top level of a chat request are neither in the published subset nor refused: each
/// of these is prepared and its field is not in the neutral turn the upstream is sent. `strict`
/// changes what the model must produce; `reasoning_content` is the field the gateway itself
/// writes into a chat answer, which a chat client replays with the assistant message.
#[test]
fn adv_translation_chat_fields_below_the_top_level_are_refused_not_dropped() {
    let translator = LlmTranslator::new();
    let model = model();
    let cases = [
        (
            "tools[].function.strict",
            "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}],\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"strict\":true,\"parameters\":{\"type\":\"object\"}}}]}",
        ),
        (
            "messages[].reasoning_content",
            "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"},{\"role\":\"assistant\",\"content\":\"Hello\",\"reasoning_content\":\"the user greets me\"},{\"role\":\"user\",\"content\":\"Again\"}]}",
        ),
        (
            "messages[].content[].cache_control",
            "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Hi\",\"cache_control\":{\"type\":\"ephemeral\"}}]}]}",
        ),
        (
            "messages[].name",
            "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"name\":\"alice\",\"content\":\"Hi\"}]}",
        ),
    ];
    let mut dropped = Vec::new();
    for (field, body) in cases {
        let refused = translator
            .prepare(&TranslationRequest {
                ingress: Wire::Chat,
                upstream: Wire::Messages,
                model: &model,
                body: body.as_bytes(),
                anthropic_beta: &[],
            })
            .err();
        if refused != Some(RefusalCode::TranslationUnsupported) {
            dropped.push(format!("{field}: {refused:?}"));
        }
    }
    assert!(
        dropped.is_empty(),
        "fields outside the subset prepared (and so dropped) instead of translation-unsupported: {dropped:#?}"
    );
}

/// The number of text deltas in the long answer: past llm's 65,536-event bound on one stream
/// (`llm_http::sse::MAX_EVENTS`), which `docs/gateway.md` does not publish.
const LONG_ANSWER_DELTAS: usize = 70_000;

/// A long answer ends with the bound's own terminal error. llm's SSE reader refuses the 65,537th
/// event of one stream, so a translated answer of more than about 65,000 output tokens (one event
/// a token on a vLLM upstream) cannot be written whole.
///
/// Re-pinned in correction round 1 of story:gateway-translation. Asserted first: the answer is
/// written whole, ending `[DONE]`. Decided instead: the bound is published in `docs/gateway.md`
/// ("Bounds of the translated path", `translated-stream-events`), and an answer past it ends with
/// the wire's terminal error naming it, `translation-too-large` on chat, never `upstream-failed`.
#[test]
fn adv_translation_an_answer_past_65536_upstream_events_ends_with_the_bounds_terminal_error() {
    let mut answer = String::from(MESSAGES_OPENING);
    let delta = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"a\"}}\n\n";
    for _ in 0..LONG_ANSWER_DELTAS {
        answer.push_str(delta);
    }
    answer.push_str(MESSAGES_CLOSING);
    let mut raw = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        answer.len()
    )
    .into_bytes();
    raw.extend_from_slice(answer.as_bytes());
    let (ran, written) = translate(
        "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}",
        raw,
    );
    assert_eq!(ran, Ok(()));
    let body = String::from_utf8_lossy(&written.body);
    let tail = &body[body.len().saturating_sub(300)..];
    let terminal = format!(
        "data: {{\"error\":{{\"code\":\"translation-too-large\",\"message\":\"{}\"}}}}\n\n",
        RefusalCode::TranslationTooLarge.reason()
    );
    assert!(
        body.ends_with(&terminal) && !body.contains("[DONE]"),
        "the answer does not end with the bound's terminal error: ...{tail}"
    );
}

/// A chat request that names no output limit, to a model with a window `w` whose upstream
/// speaks messages, reaches the upstream with a `max_tokens` that leaves room for its input:
/// below `w`. The byte relay's output clamp (`docs/gateway.md` step 7) exists because vLLM
/// answers a messages request whose `max_tokens` passes `w - n` with a 500, and the translated
/// path reads a 500 as `upstream-failed` and invalidates the target. The translated path skips
/// the clamp (design 9) and llm's projection writes `max_tokens` = the whole window, so every
/// such request asks for more than the window holds. Reached by any Runpod model (the binary
/// always declares its `max_model_len`) whose first declared wire is `messages`, from a chat or
/// responses client that names no limit, which neither wire requires.
#[test]
fn adv_translation_a_request_naming_no_limit_leaves_room_for_its_input_in_the_window() {
    let window = 32_768_u64;
    let model = model().with_max_model_len(window);
    let ran = translate_with(
        &LlmTranslator::new(),
        &model,
        "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}",
        messages_answer(),
        Duration::ZERO,
    );
    assert_eq!(ran.result, Ok(()));
    let body = ran
        .request
        .split_once("\r\n\r\n")
        .map_or("", |(_, body)| body);
    let sent: serde_json::Value = serde_json::from_str(body).unwrap();
    let max_tokens = sent["max_tokens"].as_u64().unwrap();
    assert!(
        max_tokens < window,
        "the upstream is asked for max_tokens {max_tokens} of a {window}-token window: {body}"
    );
}
