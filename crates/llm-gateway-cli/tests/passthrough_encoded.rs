//! A content-encoded passthrough answer, end to end: a loopback gateway with the binary's
//! decoder, an in-process target standing in for the Anthropic API that answers a recorded
//! streaming answer gzip-encoded, and a client that asks for gzip. The client receives the
//! target's bytes unchanged; the call record carries the tokens read from a decoded copy, the
//! decoded error text of a compressed 429, and the label of the credential, never the
//! credential. The copy of the call's bytes (story:finished-call-facts) holds the answer decoded.
//! No model is called.

use flate2::{Compression, write::GzEncoder};
use llm_gateway::{
    AnswerBytes, CallBytes, FinishedCall, FinishedCalls, Gateway, GatewayConfig, IngressAuth,
    Label, NoOwnerVerifier, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets,
    RouteInventory, StaticHeader, TargetBearer, TargetRefusal, Wire,
};
use llm_gateway_cli::Decoder;
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Duration,
};

const TOKEN: &str = "Bearer sk-ant-oat01-REDACTED-REDACTED-REDACTED";

/// The Claude Code request every exchange sends.
const REQUEST: &str = r#"{"model":"claude-sonnet-4-5-20250929","messages":[{"role":"user","content":"hi"}],"max_tokens":64,"stream":true}"#;

/// A recorded streaming answer, trimmed to the events that carry usage.
const SSE: &str = "event: message_start\n\
data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-5-20250929\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":300,\"cache_read_input_tokens\":4000,\"output_tokens\":1}}}\n\n\
event: content_block_start\n\
data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\n\
data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n\
event: content_block_stop\n\
data: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\n\
data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":42}}\n\n\
event: message_stop\n\
data: {\"type\":\"message_stop\"}\n\n";

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// A chunked gzip answer whose body is `body`, cut into chunks of 64 bytes.
fn chunked(status: &str, lines: &str, body: &[u8]) -> Vec<u8> {
    chunked_coded(status, "gzip", lines, body)
}

/// A chunked answer with the content coding `coding` whose body is `body`, cut into chunks of
/// 64 bytes.
fn chunked_coded(status: &str, coding: &str, lines: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\ncontent-encoding: {coding}\r\n{lines}transfer-encoding: chunked\r\n\r\n"
    )
    .into_bytes();
    for piece in body.chunks(64) {
        out.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
        out.extend_from_slice(piece);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"0\r\n\r\n");
    out
}

struct Scripted {
    written: Vec<u8>,
    answer: io::Cursor<Vec<u8>>,
}

impl Read for Scripted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.answer.read(buffer)
    }
}

impl Write for Scripted {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayStream for Scripted {}

struct Target(Vec<u8>);

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "api.anthropic.invalid:443"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        None
    }

    fn headers(&self) -> &[StaticHeader] {
        &[]
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(Scripted {
            written: Vec::new(),
            answer: io::Cursor::new(self.0.clone()),
        }))
    }
}

struct Source(Vec<u8>);

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(self.0.clone())))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// What one exchange left: the client's whole answer, the dashboard snapshot after it, and the
/// finished calls the port was handed.
type Exchanged = (Vec<u8>, String, Vec<(FinishedCall, Option<CallBytes>)>);

/// A gateway with the binary's decoder whose one passthrough route answers `answer`, copying a
/// call's bytes within `call_bytes` when it is given.
fn exchange_copied(answer: Vec<u8>, call_bytes: Option<usize>) -> Exchanged {
    let port = Arc::new(Port::default());
    let model = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-sonnet-4-5-20250929").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials()
    .with_model_prefix("claude-")
    .unwrap();
    let mut relay = Relay::new(vec![model], Arc::new(Source(answer)))
        .unwrap()
        .with_decoder(Arc::new(Decoder))
        .with_finished_calls(Arc::clone(&port) as Arc<dyn FinishedCalls>);
    if let Some(bound) = call_bytes {
        relay = relay.with_call_bytes(bound);
    }
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.ingress_auth = IngressAuth::None;
    let gateway = Gateway::bind_with_relay(
        config,
        Arc::new(NoOwnerVerifier),
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    gateway.mark_ready();
    let mut stream = TcpStream::connect(gateway.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "POST /v1/messages HTTP/1.1\r\nhost: 127.0.0.1\r\naccept-encoding: gzip, deflate, br, zstd\r\nanthropic-version: 2023-06-01\r\nauthorization: {TOKEN}\r\ncontent-type: application/json\r\nx-stainless-retry-count: 0\r\ncontent-length: {}\r\n\r\n{REQUEST}",
        REQUEST.len()
    )
    .unwrap();
    let mut answered = Vec::new();
    drop(stream.read_to_end(&mut answered));
    let snapshot = gateway.dashboard().snapshot();
    gateway.shutdown();
    let held = port.0.lock().unwrap().clone();
    (answered, snapshot, held)
}

/// A gateway with the binary's decoder whose one passthrough route answers `answer`; the
/// gateway's whole answer to one Claude Code request, and the dashboard snapshot after it.
fn exchange(answer: Vec<u8>) -> (Vec<u8>, String) {
    let (answered, snapshot, _) = exchange_copied(answer, None);
    (answered, snapshot)
}

/// The finished calls a port was handed, each with its copy.
#[derive(Default)]
struct Port(Mutex<Vec<(FinishedCall, Option<CallBytes>)>>);

impl FinishedCalls for Port {
    fn finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>) {
        self.0.lock().unwrap().push((call.clone(), bytes.cloned()));
    }
}

/// The one copy a call through a relay with the copy bound `bound` was handed.
fn copy_of(answer: &[u8], bound: usize) -> CallBytes {
    let (answered, _, held) = exchange_copied(answer.to_vec(), Some(bound));
    assert_eq!(
        body_of(&answered),
        body_of(answer),
        "the client's answer is unchanged"
    );
    assert_eq!(held.len(), 1, "one finished call");
    held.into_iter().next().unwrap().1.expect("a copy")
}

/// Decision D92: the copy holds a gzip answer decoded, as its client reads it, a stream as its
/// events' payloads, and the bound counts the decoded bytes.
#[test]
fn a_gzip_answer_is_copied_decoded_and_the_bound_counts_its_decoded_bytes() {
    let decoded = SSE.len();
    let encoded = gzip(SSE.as_bytes());
    assert!(encoded.len() < decoded);
    let target = chunked("200 OK", "content-type: text/event-stream\r\n", &encoded);
    let payloads: Vec<Vec<u8>> = SSE
        .split("\n\n")
        .filter_map(|event| event.lines().find_map(|line| line.strip_prefix("data: ")))
        .map(|data| data.as_bytes().to_vec())
        .collect();
    assert_eq!(payloads.len(), 6);
    let at_the_bound = REQUEST.len() + decoded;
    assert_eq!(
        copy_of(&target, at_the_bound),
        CallBytes::Copied {
            request: REQUEST.as_bytes().to_vec(),
            answer: Some(AnswerBytes::Events(payloads)),
        }
    );
    assert_eq!(
        copy_of(&target, at_the_bound - 1),
        CallBytes::OverBound {
            request_bytes: REQUEST.len() as u64,
            answer_bytes: decoded as u64,
        }
    );

    let json = br#"{"id":"msg_02","type":"message","usage":{"input_tokens":3,"output_tokens":4}}"#;
    let target = chunked("200 OK", "content-type: application/json\r\n", &gzip(json));
    assert_eq!(
        copy_of(&target, 1 << 20),
        CallBytes::Copied {
            request: REQUEST.as_bytes().to_vec(),
            answer: Some(AnswerBytes::Body(json.to_vec())),
        }
    );
}

/// Decision D92: an answer the relay's decoder cannot decode whole is handed without bytes, with
/// the request's size, the answer's size as received and its coding.
#[test]
fn an_answer_that_does_not_decode_is_handed_its_sizes_and_its_coding() {
    let mut cut = gzip(SSE.as_bytes());
    cut.truncate(cut.len() - 8);
    for (coding, body) in [
        ("gzip", b"not gzip at all".to_vec()),
        ("gzip", cut),
        ("compress", gzip(SSE.as_bytes())),
    ] {
        let target = chunked_coded(
            "200 OK",
            coding,
            "content-type: text/event-stream\r\n",
            &body,
        );
        assert_eq!(
            copy_of(&target, 1 << 20),
            CallBytes::Undecodable {
                request_bytes: REQUEST.len() as u64,
                answer_bytes: body.len() as u64,
                coding: coding.to_owned(),
            }
        );
    }
}

fn body_of(answer: &[u8]) -> &[u8] {
    let end = answer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    &answer[end + 4..]
}

#[test]
fn a_gzip_streamed_answer_reaches_the_client_unchanged_and_its_tokens_are_recorded() {
    let target = chunked(
        "200 OK",
        "content-type: text/event-stream\r\n",
        &gzip(SSE.as_bytes()),
    );
    let (answered, snapshot) = exchange(target.clone());
    assert!(answered.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(
        String::from_utf8_lossy(&answered).contains("content-encoding: gzip\r\n"),
        "the coding is passed through"
    );
    assert_eq!(
        body_of(&answered),
        body_of(&target),
        "the client receives the target's chunks byte for byte"
    );
    for field in [
        "\"input_tokens\":12",
        "\"output_tokens\":42",
        "\"cache_read_tokens\":4000",
        "\"cache_creation_tokens\":300",
        "\"usage_note\":null",
    ] {
        assert!(snapshot.contains(field), "{field} in {snapshot}");
    }
    assert!(!snapshot.contains("sk-ant-oat01"), "{snapshot}");
}

#[test]
fn a_gzip_error_body_is_recorded_decoded_and_an_undecodable_answer_is_named() {
    let error = br#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of requests has exceeded your rate limit"}}"#;
    let target = chunked(
        "429 Too Many Requests",
        "content-type: application/json\r\nx-should-retry: true\r\n",
        &gzip(error),
    );
    let (answered, snapshot) = exchange(target.clone());
    assert_eq!(body_of(&answered), body_of(&target));
    assert!(
        snapshot.contains("Number of requests has exceeded your rate limit"),
        "{snapshot}"
    );
    assert_eq!(
        snapshot.matches("\"attempt\":").count(),
        1,
        "a client that retries itself is not retried: {snapshot}"
    );

    let broken = chunked(
        "200 OK",
        "content-type: text/event-stream\r\n",
        b"not gzip at all",
    );
    let (answered, snapshot) = exchange(broken.clone());
    assert_eq!(body_of(&answered), body_of(&broken));
    assert!(
        snapshot.contains("\"usage_note\":\"undecodable(gzip)\""),
        "{snapshot}"
    );
}
