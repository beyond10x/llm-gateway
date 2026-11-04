//! story:subscription-passthrough at the library: a model marked
//! [`RelayModel::with_client_credentials`] forwards the client's `authorization`, `x-api-key`,
//! `anthropic-version` and `anthropic-beta` byte for byte, and no other client header; a model
//! without the mark forwards none of them; a gateway that authenticates its owner forwards none
//! of them even to a marked model; and a model with a name prefix serves every model name with it,
//! relayed as the client sent it (`docs/gateway.md` "The upstream").
//!
//! The target is in-process: each connection records what the gateway writes and answers one
//! canned `200`. Nothing leaves the process but the client's loopback connection.

use llm_gateway::{
    Gateway, GatewayConfig, IngressAuth, Label, NoOwnerVerifier, OwnerToken, OwnerVerifier, Relay,
    RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier,
    TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    time::Duration,
};

const OWNER: &str = "owner-token-client-credentials-0123456789abcdef";
const ANSWER: &[u8] =
    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
const CLIENT_HEADERS: &str = "Authorization: Bearer client-SUBSCRIPTION-TOKEN-0001\r\n\
                              x-api-key: CLIENT-KEY\r\n\
                              anthropic-version: 2023-06-01\r\n\
                              anthropic-beta: oauth-2025-04-20,beta-two\r\n\
                              x-claude-code-session-id: 0b5c7e5e-session\r\n\
                              x-stainless-retry-count: 0\r\n\
                              user-agent: claude-cli/2.1.318 (external, sdk-cli)\r\n";
const FORWARDED: [&str; 4] = [
    "authorization: Bearer client-SUBSCRIPTION-TOKEN-0001",
    "x-api-key: CLIENT-KEY",
    "anthropic-version: 2023-06-01",
    "anthropic-beta: oauth-2025-04-20,beta-two",
];

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

impl RelayStream for Recording {}

struct Target(Arc<Mutex<Vec<u8>>>);

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "api.anthropic.invalid:443"
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        self.0.lock().unwrap().clear();
        Ok(Box::new(Recording {
            written: Arc::clone(&self.0),
            answer: io::Cursor::new(ANSWER),
        }))
    }
}

struct Source(Arc<Mutex<Vec<u8>>>);

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(Arc::clone(&self.0))))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

struct Relayed {
    answer: String,
    request: String,
}

/// One request for `model` with [`CLIENT_HEADERS`] through a gateway whose one model is
/// `claude` (prefix `claude-` when `prefix`), marked when `marked`; the owner is authenticated
/// when `owner`.
fn relay(marked: bool, prefix: bool, owner: bool, model: &str) -> Relayed {
    let written = Arc::new(Mutex::new(Vec::new()));
    let targets: Arc<dyn RelayTargets> = Arc::new(Source(Arc::clone(&written)));
    let mut relayed = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-upstream").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap();
    if marked {
        relayed = relayed.with_client_credentials();
    }
    if prefix {
        relayed = relayed.with_model_prefix("claude-").unwrap();
    }
    let relay = Relay::new(vec![relayed], targets).unwrap();
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    let verifier: Arc<dyn OwnerVerifier> = if owner {
        Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap())
                .unwrap(),
        )
    } else {
        config.ingress_auth = IngressAuth::None;
        Arc::new(NoOwnerVerifier)
    };
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let body = format!(r#"{{"model":"{model}","max_tokens":64,"messages":[]}}"#);
    let headers = if owner {
        CLIENT_HEADERS.replace(
            "Authorization: Bearer client-SUBSCRIPTION-TOKEN-0001",
            &format!("Authorization: Bearer {OWNER}"),
        )
    } else {
        CLIENT_HEADERS.to_owned()
    };
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    stream
        .write_all(
            format!(
                "POST /v1/messages?beta=true HTTP/1.1\r\nhost: h\r\n{headers}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let mut answered = Vec::new();
    drop(stream.read_to_end(&mut answered));
    handle.shutdown();
    let request = String::from_utf8_lossy(&written.lock().unwrap()).into_owned();
    Relayed {
        answer: String::from_utf8_lossy(&answered).into_owned(),
        request,
    }
}

fn head(request: &str) -> Vec<String> {
    request
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .split("\r\n")
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_marked_model_forwards_the_four_headers_byte_for_byte_and_nothing_else() {
    let relayed = relay(true, false, false, "claude");
    assert!(relayed.answer.starts_with("HTTP/1.1 200 "), "{}", relayed.answer);
    let head = head(&relayed.request);
    for line in FORWARDED {
        assert_eq!(
            head.iter().filter(|candidate| *candidate == line).count(),
            1,
            "{line:?} is forwarded once: {head:?}"
        );
    }
    for absent in [
        "x-claude-code-session-id",
        "x-stainless-retry-count",
        "user-agent",
    ] {
        assert!(
            !head
                .iter()
                .any(|line| line.to_ascii_lowercase().starts_with(absent)),
            "{absent} is never forwarded: {head:?}"
        );
    }
    assert!(head[0].starts_with("POST /v1/messages HTTP/1.1"), "{head:?}");
}

#[test]
fn an_unmarked_model_forwards_none_of_them() {
    let relayed = relay(false, false, false, "claude");
    assert!(relayed.answer.starts_with("HTTP/1.1 200 "), "{}", relayed.answer);
    for secret in ["SUBSCRIPTION-TOKEN", "CLIENT-KEY", "anthropic-"] {
        assert!(
            !relayed.request.contains(secret),
            "{secret} reached an unmarked target: {}",
            relayed.request
        );
    }
}

#[test]
fn a_gateway_that_authenticates_its_owner_never_forwards_the_owner_credential() {
    let relayed = relay(true, false, true, "claude");
    assert!(relayed.answer.starts_with("HTTP/1.1 200 "), "{}", relayed.answer);
    for secret in [OWNER, "CLIENT-KEY", "authorization", "anthropic-beta"] {
        assert!(
            !relayed.request.contains(secret),
            "{secret} reached the target: {}",
            relayed.request
        );
    }
}

#[test]
fn a_prefix_serves_every_name_with_it_relayed_as_sent() {
    let relayed = relay(true, true, false, "claude-opus-4-7");
    assert!(relayed.answer.starts_with("HTTP/1.1 200 "), "{}", relayed.answer);
    assert!(
        relayed.request.contains(r#""model":"claude-opus-4-7""#),
        "{}",
        relayed.request
    );
    let unknown = relay(true, true, false, "gpt-5");
    assert!(
        unknown.answer.starts_with("HTTP/1.1 404 "),
        "a name without the prefix is model-unknown: {}",
        unknown.answer
    );
    assert!(unknown.answer.contains("model-unknown"), "{}", unknown.answer);
}

#[test]
fn an_empty_prefix_is_refused() {
    let model = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-upstream").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap();
    assert!(model.with_model_prefix("").is_err());
}
