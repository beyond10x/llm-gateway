//! Second adversarial pass over story:hosted-static-headers at the library, against the
//! correction in 26bbe6c: `request_head` skips a static header named like the credential header.
//! `docs/gateway.md` step 7 now says "one named like the target's own credential header is not
//! written, so the head never carries that header twice".
//!
//! The target is in-process: its connection records what the gateway writes and answers one
//! canned `200`. Nothing leaves the process but the client's loopback connection.

use llm_gateway::{
    CredentialHeader, Gateway, GatewayConfig, Label, OwnerToken, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, StaticHeader, TargetBearer,
    TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Duration,
};

const OWNER: &str = "owner-token-adversary-w10-headers-pass-2-0123";
const KEY: &str = "target-api-key-pass-2-0123456789abcdef";
const ANSWER: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";

struct Recording {
    written: Arc<Mutex<Vec<u8>>>,
    answer: io::Cursor<&'static [u8]>,
}

impl Read for Recording {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.answer.read(buffer)
    }
}

impl Write for Recording {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A target on credential header `x-api-key`, with or without a bearer, and `headers`.
struct Keyed {
    bearer: Option<TargetBearer>,
    header: CredentialHeader,
    headers: Vec<StaticHeader>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl RelayTarget for Keyed {
    fn authority(&self) -> &'static str {
        "keyed.invalid:8000"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        self.bearer.as_ref()
    }

    fn credential_header(&self) -> Option<&CredentialHeader> {
        Some(&self.header)
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.headers
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(Recording {
            written: Arc::clone(&self.written),
            answer: io::Cursor::new(ANSWER),
        }))
    }
}

struct Source {
    with_bearer: bool,
    headers: Vec<StaticHeader>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Keyed {
            bearer: self
                .with_bearer
                .then(|| TargetBearer::new(KEY.as_bytes().to_vec()).unwrap()),
            header: CredentialHeader::new("X-Api-Key").unwrap(),
            headers: self.headers.clone(),
            written: Arc::clone(&self.written),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// Relays one chat request through a target built as `with_bearer` and `headers` say, and
/// returns the head the target was written.
fn head_written(with_bearer: bool, headers: Vec<StaticHeader>, body: &str) -> String {
    let written = Arc::new(Mutex::new(Vec::new()));
    let targets: Arc<dyn RelayTargets> = Arc::new(Source {
        with_bearer,
        headers,
        written: Arc::clone(&written),
    });
    let model = RelayModel::new(
        Label::new("keyed").unwrap(),
        Label::new("served").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    let relay = Relay::new(vec![model], targets).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream
        .write_all(
            format!(
                "POST /v1/chat/completions HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    handle.shutdown();
    let answer = String::from_utf8_lossy(&answer).into_owned();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let bytes = written.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// docs/gateway.md step 7: "one named like the target's own credential header is not written".
/// The sentence holds no condition on a bearer; `request_head` skips the header only when the
/// target also has a bearer, so a target with a credential header and no bearer is sent it.
#[test]
fn adversary_pass2_a_static_header_named_like_the_credential_header_is_not_written_without_a_bearer()
 {
    let written = head_written(
        false,
        vec![StaticHeader::new("x-api-key", "static-not-the-credential").unwrap()],
        r#"{"model":"keyed","messages":[]}"#,
    );
    let head = written.split("\r\n\r\n").next().unwrap_or_default();
    let named = head
        .split("\r\n")
        .skip(1)
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        })
        .count();
    assert_eq!(
        named, 0,
        "docs/gateway.md says a static header named like the credential header is not written:\n{head}"
    );
}

/// The whole request, byte for byte, with the skip in the middle of the static headers and a
/// value at the 1024-byte bound: every other static header is written in order, the credential
/// once, and `content-length` is the body's length.
#[test]
fn adversary_pass2_the_head_with_a_skipped_header_is_exactly_the_documented_bytes() {
    let long = "v".repeat(1024);
    let body = r#"{"model":"keyed","messages":[]}"#;
    let written = head_written(
        true,
        vec![
            StaticHeader::new("a-first", "one").unwrap(),
            StaticHeader::new("X-API-KEY", "shadow").unwrap(),
            StaticHeader::new("z-last", &long).unwrap(),
        ],
        body,
    );
    let relayed = r#"{"model":"served","messages":[]}"#;
    let expected = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: keyed.invalid:8000\r\na-first: one\r\n\
         z-last: {long}\r\nx-api-key: {KEY}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{relayed}",
        relayed.len()
    );
    assert_eq!(written, expected);
}

/// The relay reads this stream as a target connection; it names no socket to shut down.
impl RelayStream for Recording {}
