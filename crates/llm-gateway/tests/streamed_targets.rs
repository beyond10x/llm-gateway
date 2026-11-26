//! story:runpod-rest-host, Acceptance 9 in the gateway crate: a target that asks to be streamed
//! (`RelayTarget::streamed`, every Runpod pod) on a relay that holds an assembler
//! (`Relay::with_assembler`) is sent every call with `stream: true`, and a call the client did not
//! stream is answered as the one body the assembler makes of the events. Every upstream is a fake
//! on loopback, port 0, that answers only its own test's calls (`crate::upstream`); the
//! assembler is this file's, which writes the events it was handed as one JSON array, so what
//! the relay hands it is visible.

use crate::upstream::{self, Upstream as Shared};
use llm_gateway::{
    Assembly, CallBytes, CallEnding, EventReader, FinishedCall, FinishedCalls, Gateway,
    GatewayConfig, GatewayHandle, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel,
    RelayStream, RelayTarget, RelayTargets, RouteInventory, StaticHeader, StreamAssembler,
    TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

/// An upstream that reads one request of its test's calls, keeps it, writes `pieces` and closes.
struct Upstream {
    authority: String,
    shared: Shared,
}

impl Upstream {
    fn start(pieces: Vec<Vec<u8>>) -> Self {
        let shared = Shared::scripted(vec![Box::new(move |connection: &mut TcpStream| {
            for piece in pieces {
                if connection.write_all(&piece).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        })]);
        Self {
            authority: shared.authority().to_owned(),
            shared,
        }
    }

    /// The request the upstream read.
    fn request(&self) -> Vec<u8> {
        let seen = self.shared.seen();
        assert!(seen.len() <= 1, "the upstream read {} requests", seen.len());
        seen.into_iter()
            .next()
            .expect("the upstream read a request")
    }

    fn body(&self) -> String {
        String::from_utf8_lossy(upstream::body_of(&self.request())).into_owned()
    }

    fn path(&self) -> Option<String> {
        upstream::head_of(&self.request())
            .split(' ')
            .nth(1)
            .map(str::to_owned)
    }
}

struct Source {
    authority: String,
    headers: Vec<StaticHeader>,
    streamed: bool,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target {
            authority: self.authority.clone(),
            headers: self.headers.clone(),
            streamed: self.streamed,
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

struct Target {
    authority: String,
    headers: Vec<StaticHeader>,
    streamed: bool,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.headers
    }

    fn streamed(&self) -> bool {
        self.streamed
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.authority)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

/// Writes `{"wire":"<wire>","events":[<each event's data>]}`, or makes nothing of a stream
/// whose last event is not `[DONE]`.
struct Joined;

struct Joining {
    wire: Wire,
    reader: EventReader,
}

impl StreamAssembler for Joined {
    fn begin(&self, wire: Wire) -> Box<dyn Assembly> {
        Box::new(Joining {
            wire,
            reader: EventReader::default(),
        })
    }
}

impl Assembly for Joining {
    fn feed(&mut self, bytes: &[u8]) -> bool {
        self.reader.feed(bytes);
        true
    }

    fn finish(mut self: Box<Self>) -> Option<Vec<u8>> {
        let events = self.reader.take();
        let (last, rest) = events.split_last()?;
        if last.as_slice() != b"[DONE]" {
            return None;
        }
        let data: Vec<String> = rest
            .iter()
            .map(|event| String::from_utf8_lossy(event).into_owned())
            .collect();
        Some(
            format!(
                "{{\"wire\":\"{}\",\"events\":[{}]}}",
                self.wire.label(),
                data.join(",")
            )
            .into_bytes(),
        )
    }
}

#[derive(Default)]
struct Port(Mutex<Vec<FinishedCall>>);

impl FinishedCalls for Port {
    fn finished(&self, call: &FinishedCall, _bytes: Option<&CallBytes>) {
        self.0.lock().unwrap().push(call.clone());
    }
}

impl Port {
    fn one(&self) -> FinishedCall {
        let deadline = Instant::now() + WAIT;
        loop {
            let held = self.0.lock().unwrap().clone();
            if !held.is_empty() || Instant::now() > deadline {
                assert_eq!(held.len(), 1, "the port was handed {held:#?}");
                return held[0].clone();
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// An open gateway whose model `m` (served as `served`, on every wire) is relayed to `upstream`,
/// a streamed target when `streamed`, on a relay that holds the assembler when `assembles`.
fn gateway(
    upstream: &Upstream,
    streamed: bool,
    assembles: bool,
    port: &Arc<Port>,
) -> GatewayHandle {
    let label = |value: &str| Label::new(value).unwrap();
    let models = vec![
        RelayModel::new(
            label("m"),
            label("served"),
            vec![Wire::Chat, Wire::Responses, Wire::Messages],
        )
        .unwrap(),
    ];
    let source = Source {
        authority: upstream.authority.clone(),
        headers: upstream.shared.headers(),
        streamed,
    };
    let mut relay = Relay::new(models, Arc::new(source))
        .unwrap()
        .with_finished_calls(Arc::clone(port) as Arc<dyn FinishedCalls>);
    if assembles {
        relay = relay.with_assembler(Arc::new(Joined));
    }
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.ingress_auth = IngressAuth::None;
    let handle = Gateway::bind_with_relay(
        config,
        Arc::new(NoOwnerVerifier),
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

fn call(handle: &GatewayHandle, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

fn chunk(piece: &str) -> Vec<u8> {
    format!("{:x}\r\n{piece}\r\n", piece.len()).into_bytes()
}

const EVENT_HEAD: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

/// A whole event stream: two events, `[DONE]`, the last chunk.
fn events() -> Vec<Vec<u8>> {
    vec![
        EVENT_HEAD.to_vec(),
        chunk("data: {\"n\":1}\n\n"),
        chunk("data: {\"n\":2,\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4}}\n\n"),
        chunk("data: [DONE]\n\n"),
        b"0\r\n\r\n".to_vec(),
    ]
}

/// What the relay answers and what the upstream was sent, for one call on `path`.
struct Run {
    answer: String,
    sent: String,
    finished: FinishedCall,
}

fn run(pieces: Vec<Vec<u8>>, streamed: bool, assembles: bool, path: &str, body: &str) -> Run {
    let upstream = Upstream::start(pieces);
    let port = Arc::new(Port::default());
    let handle = gateway(&upstream, streamed, assembles, &port);
    let answer = call(&handle, path, body);
    let finished = port.one();
    handle.shutdown();
    let sent = upstream.body();
    assert!(upstream.path().is_some());
    Run {
        answer,
        sent,
        finished,
    }
}

fn body_of(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

/// A body the relay wrote chunked, as the relay writes every relayed answer to an HTTP/1.1
/// client, decoded; a body that is not chunked, as it is.
fn dechunked(whole: &str) -> String {
    let mut body = whole;
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size, 16) else {
            return whole.to_owned();
        };
        if size == 0 {
            break;
        }
        out.push_str(&rest[..size]);
        body = &rest[size + 2..];
    }
    out
}

/// A chat call without `stream` reaches a streamed target with `stream: true` and
/// `stream_options.include_usage`, and its client gets the one assembled body, as JSON with a
/// `content-length`, the call complete and its usage read from that body.
#[test]
fn streamed_a_chat_call_without_stream_is_sent_streamed_and_answered_as_one_assembled_body() {
    let run = run(
        events(),
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m","messages":[]}"#,
    );
    assert_eq!(
        run.sent,
        r#"{"model":"served","messages":[],"stream":true,"stream_options":{"include_usage":true}}"#
    );
    assert!(run.answer.starts_with("HTTP/1.1 200 "), "{}", run.answer);
    let head = run
        .answer
        .split("\r\n\r\n")
        .next()
        .unwrap()
        .to_ascii_lowercase();
    assert!(head.contains("content-type: application/json"), "{head}");
    let expected = r#"{"wire":"chat","events":[{"n":1},{"n":2,"usage":{"prompt_tokens":3,"completion_tokens":4}}]}"#;
    assert!(
        head.contains(&format!("content-length: {}", expected.len())),
        "{head}"
    );
    assert!(!head.contains("transfer-encoding"), "{head}");
    assert_eq!(body_of(&run.answer), expected);
    assert_eq!(run.finished.ending, CallEnding::Complete);
}

/// The responses and messages wires are sent `stream: true` and no `stream_options`, and each
/// is assembled on its own wire.
#[test]
fn streamed_a_responses_or_messages_call_is_sent_stream_true_alone() {
    for (path, wire) in [("/v1/responses", "responses"), ("/v1/messages", "messages")] {
        let run = run(
            events(),
            true,
            true,
            path,
            r#"{"model":"m","stream":false,"input":"x"}"#,
        );
        assert_eq!(
            run.sent, r#"{"model":"served","stream":true,"input":"x"}"#,
            "{path}"
        );
        assert!(
            body_of(&run.answer).starts_with(&format!("{{\"wire\":\"{wire}\"")),
            "{}",
            run.answer
        );
    }
}

/// A call the client streamed is relayed as before: the body as the relay always wrote it, the
/// events unchanged and chunked. A body that names `stream_options` keeps its own.
#[test]
fn streamed_a_call_the_client_streamed_is_relayed_unchanged() {
    let run = run(
        events(),
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m","stream":true,"stream_options":{"include_usage":false}}"#,
    );
    assert_eq!(
        run.sent,
        r#"{"model":"served","stream":true,"stream_options":{"include_usage":false}}"#
    );
    assert!(run.answer.contains("data: [DONE]"), "{}", run.answer);
    assert!(
        run.answer
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
    );
    assert_eq!(run.finished.ending, CallEnding::Complete);
}

/// Controls: a target that does not ask to be streamed, and a relay without an assembler, are
/// sent the body as before; so is a token count to a streamed target.
#[test]
fn streamed_nothing_changes_without_a_streamed_target_an_assembler_or_for_a_count() {
    for (streamed, assembles) in [(false, true), (true, false)] {
        let pieces = vec![
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}"
                .to_vec(),
        ];
        let run = run(
            pieces,
            streamed,
            assembles,
            "/v1/chat/completions",
            r#"{"model":"m"}"#,
        );
        assert_eq!(run.sent, r#"{"model":"served"}"#, "{streamed} {assembles}");
        assert_eq!(dechunked(body_of(&run.answer)), "{}");
    }
    let pieces = vec![
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 19\r\n\r\n{\"input_tokens\":12}"
            .to_vec(),
    ];
    // A count hands no finished call, so it is run without waiting for one.
    let upstream = Upstream::start(pieces);
    let port = Arc::new(Port::default());
    let handle = gateway(&upstream, true, true, &port);
    let answer = call(&handle, "/v1/messages/count_tokens", r#"{"model":"m"}"#);
    handle.shutdown();
    assert_eq!(upstream.body(), r#"{"model":"served"}"#);
    assert_eq!(dechunked(body_of(&answer)), "{\"input_tokens\":12}");
}

/// Any process on the host reaches the upstream's loopback port: a request from no call of this
/// test takes none of its pieces, and the gateway's call still gets them.
#[test]
fn streamed_a_foreign_connection_to_the_upstream_takes_none_of_its_pieces() {
    let upstream = Upstream::start(events());
    let foreign = crate::upstream::foreign_request(&upstream.authority);
    let port = Arc::new(Port::default());
    let handle = gateway(&upstream, true, true, &port);
    let answer = call(&handle, "/v1/chat/completions", r#"{"model":"m"}"#);
    handle.shutdown();
    assert!(
        foreign.is_empty(),
        "a foreign connection took the upstream's pieces: {}",
        String::from_utf8_lossy(&foreign)
    );
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(port.one().ending, CallEnding::Complete);
}

/// An answer that is not a 2xx event stream (a model's error) is relayed as it came.
#[test]
fn streamed_an_answer_that_is_no_event_stream_is_relayed_as_it_came() {
    let pieces = vec![
        b"HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"error\":1}"
            .to_vec(),
    ];
    let run = run(
        pieces,
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m"}"#,
    );
    assert!(run.answer.starts_with("HTTP/1.1 400 "), "{}", run.answer);
    assert_eq!(dechunked(body_of(&run.answer)), "{\"error\":1}");
    assert_eq!(run.finished.ending, CallEnding::Complete);
}

/// An event stream whose connection closes before its end (the proxy's bound, or the pod gone)
/// is the upstream cut: the head is written, the answer ends without its body, and the call
/// ends `UpstreamCut`.
#[test]
fn streamed_an_event_stream_cut_before_its_end_is_the_upstream_cut() {
    let pieces = vec![EVENT_HEAD.to_vec(), chunk("data: {\"n\":1}\n\n")];
    let run = run(
        pieces,
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m"}"#,
    );
    assert!(run.answer.starts_with("HTTP/1.1 200 "), "{}", run.answer);
    assert!(!run.answer.ends_with("0\r\n\r\n"), "{}", run.answer);
    assert!(
        !run.answer.contains("\"n\""),
        "no part of an unassembled answer: {}",
        run.answer
    );
    assert_eq!(run.finished.ending, CallEnding::UpstreamCut);
}

/// The Runpod proxy's `524` is relayed, and the call ends `UpstreamCut`.
#[test]
fn streamed_a_524_of_a_streamed_target_is_relayed_and_ends_as_the_upstream_cut() {
    let pieces = vec![
        b"HTTP/1.1 524 \r\ncontent-type: text/plain\r\ncontent-length: 21\r\n\r\nA timeout occurred..."
            .to_vec(),
    ];
    let run = run(
        pieces,
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m"}"#,
    );
    assert!(run.answer.starts_with("HTTP/1.1 524 "), "{}", run.answer);
    assert_eq!(run.finished.ending, CallEnding::UpstreamCut);
}

/// A stream the assembler makes no answer of is `upstream-failed`.
#[test]
fn streamed_a_stream_the_assembler_cannot_make_one_answer_of_is_upstream_failed() {
    let pieces = vec![
        EVENT_HEAD.to_vec(),
        chunk("data: {\"n\":1}\n\n"),
        b"0\r\n\r\n".to_vec(),
    ];
    let run = run(
        pieces,
        true,
        true,
        "/v1/chat/completions",
        r#"{"model":"m"}"#,
    );
    assert!(run.answer.starts_with("HTTP/1.1 502 "), "{}", run.answer);
    assert!(
        run.answer.contains("\"code\":\"upstream-failed\""),
        "{}",
        run.answer
    );
}
