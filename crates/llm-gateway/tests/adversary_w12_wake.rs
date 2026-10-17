//! Adversarial pass over story:model-wake-and-status: the lifecycle answer's shape as
//! docs/gateway.md publishes it ("a value the source did not report absent, never empty").
//! The target source is a stub; no target is handed out and no connection leaves the gateway.

use llm_gateway::{
    Gateway, GatewayConfig, Label, ModelPhase, ModelStatus, OwnerToken, Relay, RelayModel,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// A source whose status and wake report an empty sub-phase and an empty profile.
struct Empty;

impl Empty {
    fn status() -> ModelStatus {
        ModelStatus {
            phase: ModelPhase::Starting,
            sub_phase: Some(String::new()),
            profile: Some(String::new()),
        }
    }
}

impl RelayTargets for Empty {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}

    fn wake(&self, _alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        Some(Ok(Self::status()))
    }

    fn status(&self, _alias: &str) -> Option<ModelStatus> {
        Some(Self::status())
    }
}

fn send(address: SocketAddr, method: &str, path: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// docs/gateway.md, "The lifecycle routes": the `ModelStatus` keys appear "a value the source
/// did not report absent, never empty". `lifecycle_body` writes any `Some` as it is, so an
/// empty sub-phase or profile reaches the owner as `""`.
#[test]
fn adversary_w12_an_empty_sub_phase_or_profile_is_left_out_never_answered_empty() {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let model = RelayModel::new(
        Label::new("small").unwrap(),
        Label::new("small").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    let relay = Relay::new(vec![model], Arc::new(Empty)).unwrap();
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let wake = send(handle.local_addr(), "POST", "/v1/models/small/wake");
    let status = send(handle.local_addr(), "GET", "/v1/models/small/status");
    for answer in [&wake, &status] {
        let body = answer.split_once("\r\n\r\n").map_or("", |(_, body)| body);
        assert_eq!(
            body, "{\"model\":\"small\",\"phase\":\"starting\"}",
            "{answer}"
        );
    }
}
