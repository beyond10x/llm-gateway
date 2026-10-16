//! story:gateway-translation, correction round 1: the bounds of the translated path, the output
//! limit a messages upstream is asked for, and a non-streamed client that goes away.
//!
//! Each case drives the binary's `LlmTranslator` through the port as the relay does, against a
//! loopback upstream. Nothing leaves the machine.

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

const CONTRACT: &str = include_str!("../../../docs/gateway.md");
const WAIT: Duration = Duration::from_secs(30);

/// Three events: the message, a text block, and its delta `Hel`.
const OPENING: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_up\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"served\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
);

/// Three events that end the message.
const CLOSING: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

const DELTA: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"a\"}}\n\n";

const STREAMED: &str =
    "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
const WHOLE: &str = "{\"model\":\"code\",\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
/// A streamed request offering the `write` tool a measured answer may call.
const WITH_TOOLS: &str = "{\"model\":\"code\",\"stream\":true,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}],\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"write\",\"parameters\":{\"type\":\"object\"}}}]}";

struct Loopback(String);

impl RelayTarget for Loopback {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::other("the translator opens its own connection"))
    }
}

/// The client's side, recorded; `gone` is what [`TranslatedAnswer::gone`] reports.
#[derive(Default)]
struct Recorded {
    head: Option<(u16, String, AnswerFraming)>,
    body: Vec<u8>,
    gone: bool,
    asked_gone: usize,
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
        self.body.extend_from_slice(bytes);
        self.head.is_some()
    }

    fn gone(&mut self) -> bool {
        self.asked_gone += 1;
        self.gone
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

/// What an upstream saw: the request, and whether the gateway closed the connection while the
/// upstream still held the rest of its answer.
struct Seen {
    request: String,
    cut: bool,
}

/// Answers one connection with an event stream: `sent`, then, when `waits`, waits for the
/// gateway to close the connection before it would send the rest.
fn upstream(body: String, waits: bool) -> (SocketAddr, JoinHandle<Seen>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let thread = thread::spawn(move || {
        let (mut connection, _) = listener.accept().unwrap();
        connection.set_read_timeout(Some(WAIT)).unwrap();
        let request = String::from_utf8_lossy(&read_request(&mut connection)).into_owned();
        let length = if waits {
            body.len() + CLOSING.len()
        } else {
            body.len()
        };
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
        );
        drop(connection.write_all(head.as_bytes()));
        drop(connection.write_all(body.as_bytes()));
        drop(connection.flush());
        let mut cut = false;
        if waits {
            // A read that times out is a gateway that kept the connection: not cut.
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut byte = [0_u8; 1];
            cut = match connection.read(&mut byte) {
                Ok(0) => true,
                Ok(_) => false,
                Err(error) => !matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ),
            };
            if !cut {
                drop(connection.write_all(CLOSING.as_bytes()));
            }
        }
        drop(connection.shutdown(Shutdown::Both));
        Seen { request, cut }
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

/// Prepares `request` on the chat wire for `model`, runs it against an upstream answering `body`.
fn run(
    model: &RelayModel,
    request: &str,
    body: String,
    held: bool,
    answer: &mut Recorded,
) -> (Result<(), TranslationFailure>, Seen) {
    let prepared = LlmTranslator::new()
        .prepare(&TranslationRequest {
            ingress: Wire::Chat,
            upstream: Wire::Messages,
            model,
            body: request.as_bytes(),
        })
        .unwrap_or_else(|code| panic!("prepare refused {code:?}"));
    let (address, thread) = upstream(body, held);
    let ran = prepared.run(&Loopback(address.to_string()), answer);
    (ran, thread.join().unwrap())
}

/// How a streamed chat answer to `body` ended: `true` whole, `false` with the bound's terminal
/// error; anything else fails.
fn ended_whole(body: String) -> bool {
    let mut answer = Recorded::default();
    let (ran, _) = run(&model(None), WITH_TOOLS, body, false, &mut answer);
    assert_eq!(ran, Ok(()));
    let written = String::from_utf8_lossy(&answer.body).into_owned();
    let terminal = format!(
        "data: {{\"error\":{{\"code\":\"translation-too-large\",\"message\":\"{}\"}}}}\n\n",
        RefusalCode::TranslationTooLarge.reason()
    );
    let tail = &written[written.len().saturating_sub(300)..];
    match (
        written.ends_with("data: [DONE]\n\n"),
        written.ends_with(&terminal),
    ) {
        (true, false) => true,
        (false, true) => {
            assert!(!written.contains("[DONE]"), "...{tail}");
            false
        }
        _ => panic!("the answer ends neither whole nor with the bound's error: ...{tail}"),
    }
}

/// A stream of exactly `events` events.
fn events(events: usize) -> String {
    let mut body = String::from(OPENING);
    for _ in 0..events - 6 {
        body.push_str(DELTA);
    }
    body.push_str(CLOSING);
    body
}

/// SSE comment lines of exactly `bytes` bytes, none longer than a million.
fn filler(mut bytes: usize) -> String {
    let mut out = String::with_capacity(bytes);
    while bytes > 0 {
        let line = if bytes > 1_000_003 { 1_000_002 } else { bytes };
        assert!(line >= 2);
        out.push(':');
        out.push_str(&"a".repeat(line - 2));
        out.push('\n');
        bytes -= line;
    }
    out
}

/// A stream of exactly `bytes` bytes.
fn stream_bytes(bytes: usize) -> String {
    format!(
        "{OPENING}{}{CLOSING}",
        filler(bytes - OPENING.len() - CLOSING.len())
    )
}

/// A stream holding one event whose data, two `data:` lines joined, is exactly `bytes` bytes.
fn event_bytes(bytes: usize) -> String {
    let prefix = "{\"type\":\"content_block_delta\",\"index\":0,";
    let suffix = "\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}";
    let padding = bytes - prefix.len() - suffix.len() - 1;
    let first = " ".repeat(padding / 2);
    let second = " ".repeat(padding - padding / 2);
    format!(
        "{OPENING}event: content_block_delta\ndata: {prefix}{first}\ndata: {second}{suffix}\n\n{CLOSING}"
    )
}

/// A stream holding one `write` tool call whose arguments are exactly `bytes` bytes, in
/// fragments of at most 8192.
fn tool_argument_bytes(bytes: usize) -> String {
    let start = OPENING.split_once("event: content_block_start").unwrap().0;
    let mut fragments = vec!["{\"content\":\"".to_owned()];
    let mut left = bytes - "{\"content\":\"\"}".len();
    while left > 0 {
        let piece = left.min(8192);
        fragments.push("a".repeat(piece));
        left -= piece;
    }
    fragments.push("\"}".to_owned());
    let mut body = format!(
        "{start}event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"write\",\"input\":{{}}}}}}\n\n"
    );
    for fragment in fragments {
        let fragment = serde_json::to_string(&fragment).unwrap();
        body.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{fragment}}}}}\n\n"
        ));
    }
    body.push_str(&CLOSING.replace("end_turn", "tool_use"));
    body
}

/// A stream holding one comment line of exactly `bytes` bytes.
fn line_bytes(bytes: usize) -> String {
    format!("{OPENING}:{}\n{CLOSING}", "a".repeat(bytes - 1))
}

/// The rows of `docs/gateway.md` "Bounds of the translated path", as `(name, value)`.
fn published() -> Vec<(String, usize)> {
    CONTRACT
        .split_once("\n## Bounds of the translated path\n")
        .expect("docs/gateway.md has no Bounds of the translated path section")
        .1
        .split("\n## ")
        .next()
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 3, "unexpected bound row: {line}");
            (
                cells[0].trim_matches('`').to_owned(),
                cells[2]
                    .parse()
                    .unwrap_or_else(|_| panic!("unparsable bound: {line}")),
            )
        })
        .collect()
}

/// Every bound `docs/gateway.md` publishes for the translated path, and no other, holds at
/// exactly its number: an answer at it is written whole, one past it ends with the wire's
/// terminal error naming the bound, `translation-too-large`.
#[test]
fn translation_bound_every_published_bound_of_the_translated_path_flips_at_its_number() {
    let published = published();
    let names: Vec<&str> = published.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "translated-stream-events",
            "translated-stream-bytes",
            "translated-line-bytes",
            "translated-event-bytes",
            "translated-tool-argument-bytes"
        ]
    );
    for (name, value) in &published {
        let stream: fn(usize) -> String = match name.as_str() {
            "translated-stream-events" => events,
            "translated-stream-bytes" => stream_bytes,
            "translated-line-bytes" => line_bytes,
            "translated-event-bytes" => event_bytes,
            _ => tool_argument_bytes,
        };
        assert!(
            ended_whole(stream(*value)),
            "{name} refuses its own {value}"
        );
        assert!(
            !ended_whole(stream(*value + 1)),
            "{name} admits {} ",
            value + 1
        );
    }
}

/// A non-streamed answer past a bound is refused `translation-too-large` before any output.
#[test]
fn translation_bound_a_whole_answer_past_a_bound_is_refused_translation_too_large() {
    let mut answer = Recorded::default();
    let (ran, _) = run(&model(None), WHOLE, events(65_537), false, &mut answer);
    assert_eq!(
        ran,
        Err(TranslationFailure::Refused(
            RefusalCode::TranslationTooLarge
        ))
    );
    assert!(answer.head.is_none());
}

/// A request naming no output limit asks a messages upstream for `min(16384, w / 2)` of a
/// window `w`, 16384 of the 200000 a model without a window is translated under (re-pinned in
/// correction round 2 from the capabilities' 200000), and a named limit unchanged.
#[test]
fn translation_bound_a_messages_upstream_is_asked_for_half_the_window_at_most_16384() {
    let limit = |window: Option<u64>, request: &str| {
        let mut answer = Recorded::default();
        let (ran, seen) = run(&model(window), request, events(6), false, &mut answer);
        assert_eq!(ran, Ok(()));
        let body = seen
            .request
            .split_once("\r\n\r\n")
            .map_or("", |(_, body)| body);
        let sent: serde_json::Value = serde_json::from_str(body).unwrap();
        sent["max_tokens"].as_u64().unwrap()
    };
    assert_eq!(limit(Some(65_536), STREAMED), 16_384);
    assert_eq!(limit(Some(32_768), STREAMED), 16_384);
    assert_eq!(limit(Some(8_192), STREAMED), 4_096);
    assert_eq!(limit(None, STREAMED), 16_384);
    let named = "{\"model\":\"code\",\"stream\":true,\"max_tokens\":8000,\"messages\":[{\"role\":\"user\",\"content\":\"Hi\"}]}";
    assert_eq!(limit(Some(32_768), named), 8_000);
}

/// A non-streamed request whose client has gone is noticed between two events: the turn is
/// cancelled, the upstream connection closes before the upstream's answer ends, and nothing is
/// written.
#[test]
fn translation_bound_a_non_streamed_client_that_has_gone_cancels_the_turn() {
    let mut answer = Recorded {
        gone: true,
        ..Recorded::default()
    };
    let (ran, seen) = run(&model(None), WHOLE, OPENING.to_owned(), true, &mut answer);
    assert_eq!(ran, Ok(()));
    assert!(seen.cut, "the upstream connection stayed open");
    assert!(answer.head.is_none());
    assert!(answer.asked_gone > 0);
}

/// A client still there is asked, and answered whole.
#[test]
fn translation_bound_a_non_streamed_client_still_there_is_answered() {
    let mut answer = Recorded::default();
    let (ran, seen) = run(
        &model(None),
        WHOLE,
        format!("{OPENING}{CLOSING}"),
        false,
        &mut answer,
    );
    assert_eq!(ran, Ok(()));
    assert!(!seen.cut);
    assert!(answer.asked_gone > 0);
    let (status, content_type, _) = answer.head.clone().unwrap();
    assert_eq!((status, content_type.as_str()), (200, "application/json"));
}
