//! Adversarial pass over story:hosted-static-headers at the library: `RelayTarget::headers`
//! written by `request_head`, against `docs/gateway.md` step 7, which says "no static header
//! can end its line, replace or repeat a header the gateway writes, or carry `authorization`".
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

const OWNER: &str = "owner-token-adversary-w10-headers-0123456789";
const KEY: &str = "target-api-key-0123456789abcdef";
const ANSWER: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";

/// One connection: records every byte written, answers [`ANSWER`].
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

/// An `api-key` target on `x-api-key` whose static headers also name `x-api-key`.
struct Shadowing {
    bearer: TargetBearer,
    header: CredentialHeader,
    headers: Vec<StaticHeader>,
    written: Arc<Mutex<Vec<u8>>>,
}

impl RelayTarget for Shadowing {
    fn authority(&self) -> &'static str {
        "shadowing.invalid:8000"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        Some(&self.bearer)
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
    written: Arc<Mutex<Vec<u8>>>,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Shadowing {
            bearer: TargetBearer::new(KEY.as_bytes().to_vec()).unwrap(),
            header: CredentialHeader::new("x-api-key").unwrap(),
            headers: vec![StaticHeader::new("X-Api-Key", "inline-not-the-credential").unwrap()],
            written: Arc::clone(&self.written),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// docs/gateway.md step 7: a static header cannot repeat a header the gateway writes. The
/// credential header is one the gateway writes, and `StaticHeader` with `RelayTarget::headers`
/// do not stop a target naming it: the head carries `x-api-key` twice. Only the binary's
/// `HostedEndpoint::with_headers` refuses it.
#[test]
fn adversary_a_static_header_cannot_repeat_the_credential_header_the_gateway_writes() {
    let written = Arc::new(Mutex::new(Vec::new()));
    let targets: Arc<dyn RelayTargets> = Arc::new(Source {
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
    let body = r#"{"model":"keyed","messages":[]}"#;
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
    let written = String::from_utf8_lossy(&written.lock().unwrap()).into_owned();
    let head = written.split("\r\n\r\n").next().unwrap_or_default();
    let keyed = head
        .split("\r\n")
        .skip(1)
        .filter(|line| {
            line.split_once(':')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
        })
        .count();
    assert_eq!(keyed, 1, "the head repeats the credential header:\n{head}");
}
