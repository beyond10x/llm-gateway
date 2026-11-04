//! A content-encoded passthrough answer, end to end: a loopback gateway with the binary's
//! decoder, an in-process target standing in for the Anthropic API that answers a recorded
//! streaming answer gzip-encoded, and a client that asks for gzip. The client receives the
//! target's bytes unchanged; the call record carries the tokens read from a decoded copy, the
//! decoded error text of a compressed 429, and the label of the credential, never the
//! credential. No model is called.

use flate2::{Compression, write::GzEncoder};
use llm_gateway::{
    Gateway, GatewayConfig, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, StaticHeader, TargetBearer, TargetRefusal, Wire,
};
use llm_gateway_cli::Decoder;
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

const TOKEN: &str = "Bearer sk-ant-oat01-REDACTED-REDACTED-REDACTED";

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

/// A chunked answer whose body is `body`, cut into chunks of 64 bytes.
fn chunked(status: &str, lines: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\ncontent-encoding: gzip\r\n{lines}transfer-encoding: chunked\r\n\r\n"
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

/// A gateway with the binary's decoder whose one passthrough route answers `answer`; the
/// gateway's whole answer to one Claude Code request, and the dashboard snapshot after it.
fn exchange(answer: Vec<u8>) -> (Vec<u8>, String) {
    let model = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-sonnet-4-5-20250929").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials()
    .with_model_prefix("claude-")
    .unwrap();
    let relay = Relay::new(vec![model], Arc::new(Source(answer)))
        .unwrap()
        .with_decoder(Arc::new(Decoder));
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
    let body = r#"{"model":"claude-sonnet-4-5-20250929","messages":[{"role":"user","content":"hi"}],"max_tokens":64,"stream":true}"#;
    let mut stream = TcpStream::connect(gateway.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "POST /v1/messages HTTP/1.1\r\nhost: 127.0.0.1\r\naccept-encoding: gzip, deflate, br, zstd\r\nanthropic-version: 2023-06-01\r\nauthorization: {TOKEN}\r\ncontent-type: application/json\r\nx-stainless-retry-count: 0\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answered = Vec::new();
    drop(stream.read_to_end(&mut answered));
    let snapshot = gateway.dashboard().snapshot();
    gateway.shutdown();
    (answered, snapshot)
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
