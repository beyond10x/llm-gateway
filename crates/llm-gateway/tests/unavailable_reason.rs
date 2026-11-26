//! story:runpod-rest-host, Acceptance 4 in the gateway crate: a target source that says why it
//! has no target for a model (`RelayTargets::unavailable_reason`) has a call and a wake of that
//! model answered `target-unavailable` whose message is the fixed text, `: ` and that reason. A
//! source that says nothing keeps the fixed message.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, ModelStatus, NoOwnerVerifier, Relay,
    RelayModel, RelayTarget, RelayTargets, RouteInventory, TargetRefusal, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::Arc,
    time::Duration,
};

struct Down(Option<&'static str>);

impl RelayTargets for Down {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}

    fn wake(&self, _alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        Some(Err(TargetRefusal::Unavailable))
    }

    fn unavailable_reason(&self, alias: &str) -> Option<String> {
        self.0.filter(|_| alias == "m").map(str::to_owned)
    }
}

fn gateway(reason: Option<&'static str>) -> GatewayHandle {
    let models = vec![
        RelayModel::new(
            Label::new("m").unwrap(),
            Label::new("served").unwrap(),
            vec![Wire::Chat],
        )
        .unwrap(),
    ];
    let relay = Relay::new(models, Arc::new(Down(reason))).unwrap();
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

fn send(handle: &GatewayHandle, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let length = if body.is_empty() {
        String::new()
    } else {
        format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
    };
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\n{length}\r\n{body}"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

#[test]
fn unavailable_a_call_and_a_wake_name_the_reason_the_source_gives() {
    let handle = gateway(Some("provider runpod was not reachable at start"));
    let call = send(
        &handle,
        "/v1/chat/completions",
        r#"{"model":"m","messages":[]}"#,
    );
    let wake = send(&handle, "/v1/models/m/wake", "");
    handle.shutdown();
    let expected = r#"{"error":{"code":"target-unavailable","message":"no model target is available: provider runpod was not reachable at start"}}"#;
    for answer in [&call, &wake] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert_eq!(body(answer), expected);
    }
}

#[test]
fn unavailable_a_source_that_gives_no_reason_keeps_the_fixed_message() {
    let handle = gateway(None);
    let call = send(
        &handle,
        "/v1/chat/completions",
        r#"{"model":"m","messages":[]}"#,
    );
    let wake = send(&handle, "/v1/models/m/wake", "");
    handle.shutdown();
    let expected =
        r#"{"error":{"code":"target-unavailable","message":"no model target is available"}}"#;
    for answer in [&call, &wake] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert_eq!(body(answer), expected);
    }
}
