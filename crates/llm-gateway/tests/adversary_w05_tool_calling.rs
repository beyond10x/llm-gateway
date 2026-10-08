//! Adversarial pass over `story:model-tool-calling`: the `tools-not-served` refusal, attacked over
//! a real socket. The target source hands out no target and records every `acquire`, so a
//! request the relay lets through answers `target-unavailable` (503) and counts one acquisition,
//! and a refused one counts none. Nothing leaves the loopback interface.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, ToolCalling, Wire,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

const OWNER: &str = "owner-token-adversary-w05-tools-0123456789";
const WAIT: Duration = Duration::from_secs(20);

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

#[derive(Default)]
struct Source {
    acquired: Mutex<Vec<String>>,
}

impl Source {
    fn acquired(&self) -> usize {
        self.acquired.lock().unwrap().len()
    }
}

impl RelayTargets for Source {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.acquired.lock().unwrap().push(alias.to_string());
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

struct Relayed {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Relayed {
    fn start(source: &Arc<Source>) -> Self {
        let verifier = Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
        );
        let text = RelayModel::new(label("text"), label("served-text"), vec![Wire::Chat])
            .unwrap()
            .with_tool_calling(ToolCalling::Absent);
        let code = RelayModel::new(
            label("code"),
            label("served-code"),
            vec![Wire::Chat, Wire::Responses, Wire::Messages],
        )
        .unwrap()
        .with_tool_calling(ToolCalling::Parsed);
        let targets: Arc<dyn RelayTargets> = Arc::clone(source) as Arc<dyn RelayTargets>;
        let relay = Relay::new(vec![text, code], targets).unwrap();
        let handle = Gateway::bind_with_relay(
            GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
            verifier,
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
        }
    }

    fn exchange(&self, raw: &[u8]) -> (u16, String) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.set_write_timeout(Some(WAIT)).unwrap();
        drop(stream.write_all(raw));
        drop(stream.flush());
        let mut received = Vec::new();
        drop(stream.read_to_end(&mut received));
        let text = String::from_utf8_lossy(&received).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or(0);
        let code = text
            .find("\"code\":\"")
            .map(|start| {
                let rest = &text[start + 8..];
                rest[..rest.find('"').unwrap_or(0)].to_string()
            })
            .unwrap_or_default();
        (status, code)
    }

    fn post(&self, path: &str, body: &str) -> (u16, String) {
        self.exchange(
            format!(
                "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\
                 content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
    }
}

impl Drop for Relayed {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

/// A key is compared after JSON unescaping, as the target's own JSON reader does: a `tools`
/// spelled with an escape still offers tools, and is refused without a target.
#[test]
fn adversary_w05_an_escaped_tools_key_is_still_refused_without_a_target() {
    let source = Arc::new(Source::default());
    let gateway = Relayed::start(&source);
    for body in [
        "{\"model\":\"text\",\"t\\u006fols\":[{}]}",
        "{\"model\":\"text\",\"\\u0074ools\":[{\"type\":\"function\"}]}",
        "{\"model\":\"text\" ,\n\"tools\"\n:\n[\n{}\n]\n}",
        "{\"model\":\"text\",\"tools\":[null]}",
        "{\"model\":\"text\",\"tools\":[[]]}",
        "{\"model\":\"text\",\"tools\":[{}],\"tools\":[]}",
    ] {
        assert_eq!(
            gateway.post("/v1/chat/completions", body),
            (400, "tools-not-served".to_string()),
            "{body}"
        );
    }
    assert_eq!(source.acquired(), 0);
}

/// A chunked body is decoded before the scan, so the refusal does not depend on framing.
#[test]
fn adversary_w05_a_chunked_tool_request_is_refused_without_a_target() {
    let source = Arc::new(Source::default());
    let gateway = Relayed::start(&source);
    let body = "{\"model\":\"text\",\"tools\":[{\"type\":\"function\"}]}";
    let (first, second) = body.split_at(20);
    let raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\
         transfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
        first.len(),
        second.len()
    );
    assert_eq!(
        gateway.exchange(raw.as_bytes()),
        (400, "tools-not-served".to_string())
    );
    assert_eq!(source.acquired(), 0);
}

/// Refusal order: authentication first, then the wire (`wire-not-served`) before tools, as
/// `docs/gateway.md` "The relay" step 5 orders them.
#[test]
fn adversary_w05_tools_not_served_comes_after_auth_and_wire_not_served() {
    let source = Arc::new(Source::default());
    let gateway = Relayed::start(&source);
    let body = "{\"model\":\"text\",\"tools\":[{}]}";
    let unauthenticated = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(
        gateway.exchange(unauthenticated.as_bytes()),
        (401, "credential-absent".to_string())
    );
    assert_eq!(
        gateway.post("/v1/messages", body),
        (400, "wire-not-served".to_string())
    );
    assert_eq!(source.acquired(), 0);
}

/// A `Parsed` model is asked for a target on every wire it declares when tools are offered.
#[test]
fn adversary_w05_a_parsed_model_asks_for_a_target_on_every_wire() {
    let source = Arc::new(Source::default());
    let gateway = Relayed::start(&source);
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        assert_eq!(
            gateway.post(path, "{\"model\":\"code\",\"tools\":[{}]}"),
            (503, "target-unavailable".to_string()),
            "{path}"
        );
    }
    assert_eq!(source.acquired(), 3);
}
