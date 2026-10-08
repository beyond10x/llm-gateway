//! Adversary cases for `story:public-model-listing`, pass 1: `GET /` (row R1) against
//! acceptance 2, "anything else, or no `User-Agent`: plain text".

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayTarget,
    RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, ToolCalling, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

const OWNER_SECRET: &str = "owner-token-adversary-listing-0123456789ab";

struct NoTargets;

impl RelayTargets for NoTargets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        panic!("a public route asked the target source for a target");
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn gateway() -> GatewayHandle {
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served-code").unwrap(),
        Wire::ALL.to_vec(),
    )
    .unwrap()
    .with_tool_calling(ToolCalling::Parsed)
    .with_context_window(65_536);
    let relay = Relay::new(vec![model], Arc::new(NoTargets)).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(5);
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

/// The whole answer, as bytes, to one raw request head.
fn exchange(handle: &GatewayHandle, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    String::from_utf8_lossy(&raw).into_owned()
}

/// RFC 9110 section 5.5 admits obs-text (0x80-0xFF) in a field value, but the gateway reads a
/// request head only as UTF-8 and refuses any other head as `request-malformed`, on every route
/// (`docs/gateway.md`, the causes of `request-malformed`). So a `user-agent` that is not UTF-8
/// never reaches the selection rules of `GET /`: it gets the documented refusal, not one of the
/// four answers. No client the setup text addresses sends one. If the head reader starts to admit
/// obs-text, this case must change to expect the plain-text answer for "anything else".
#[test]
fn adversary_r1_a_user_agent_that_is_not_utf8_is_request_malformed() {
    let handle = gateway();
    let request = b"GET / HTTP/1.1\r\nhost: gateway\r\nuser-agent: curl/8.0 (Z\xfcrich)\r\n\r\n";
    let answer = exchange(&handle, request);
    handle.shutdown();
    assert!(answer.starts_with("HTTP/1.1 400 "), "{answer}");
    assert!(
        answer.contains("\"code\":\"request-malformed\""),
        "{answer}"
    );
    assert!(!answer.contains("upstream_name"), "{answer}");
}
