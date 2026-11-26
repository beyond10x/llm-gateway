//! Adversarial pass over `story:finished-call-facts` (wave 2026-11-26-w05, unit U2).
//!
//! `CallEnding` names its precedence: refused, then ended in its wire's terminal error, then cut
//! short by its upstream, then left by its client, then complete (`finished.rs`, docs/gateway.md
//! "Finished calls": "`Refused(code)`, else `WireError` ..., else `UpstreamCut`"). An answer
//! that carried its wire's terminal error and whose upstream then closed without ending the body
//! is both; by that precedence it is a `WireError`. Every upstream here is a fake on loopback,
//! port 0, that answers only its own test's call (`crate::upstream`): it writes its pieces and
//! closes.

use crate::upstream::{self, Upstream};
use llm_gateway::{
    CallBytes, CallEnding, FinishedCall, FinishedCalls, Gateway, GatewayConfig, GatewayHandle,
    IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel, RelayTarget, RelayTargets,
    RouteInventory, TargetRefusal, Wire,
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const WAIT: Duration = Duration::from_secs(10);

/// An upstream that reads one request of its test's call, writes `pieces` and closes.
fn piecewise(pieces: Vec<Vec<u8>>) -> Upstream {
    Upstream::scripted(vec![Box::new(move |connection: &mut TcpStream| {
        for piece in pieces {
            if connection.write_all(&piece).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    })])
}

struct Source(upstream::Target);

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(self.0.clone()))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

#[derive(Default)]
struct Port(Mutex<Vec<(FinishedCall, Option<CallBytes>)>>);

impl FinishedCalls for Port {
    fn finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>) {
        self.0.lock().unwrap().push((call.clone(), bytes.cloned()));
    }
}

impl Port {
    fn one(&self) -> FinishedCall {
        let deadline = Instant::now() + WAIT;
        loop {
            let held = self.0.lock().unwrap().clone();
            if !held.is_empty() || Instant::now() > deadline {
                assert_eq!(held.len(), 1, "the port was handed {held:#?}");
                return held[0].0.clone();
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// An open gateway (no ingress credential) over `upstream`: `code` on messages through the byte
/// relay, `sub` on messages through the client-credential passthrough.
fn gateway(upstream: &Upstream, port: &Arc<Port>) -> GatewayHandle {
    let label = |value: &str| Label::new(value).unwrap();
    let models = vec![
        RelayModel::new(label("code"), label("served-code"), vec![Wire::Messages]).unwrap(),
        RelayModel::new(label("sub"), label("served-sub"), vec![Wire::Messages])
            .unwrap()
            .with_client_credentials(),
    ];
    let relay = Relay::new(models, Arc::new(Source(upstream.target(WAIT))))
        .unwrap()
        .with_finished_calls(Arc::clone(port) as Arc<dyn FinishedCalls>);
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

fn call(handle: &GatewayHandle, body: &str, headers: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST /v1/messages HTTP/1.1\r\nhost: gateway\r\n{headers}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    answer
}

fn chunk(piece: &str) -> Vec<u8> {
    format!("{:x}\r\n{piece}\r\n", piece.len()).into_bytes()
}

/// A chunked messages stream: its first event, then the wire's terminal error event, then the
/// connection closes without the last chunk.
fn error_then_cut() -> Vec<Vec<u8>> {
    vec![
        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n"
            .to_vec(),
        chunk(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":9,\"output_tokens\":1}}}\n\n",
        ),
        chunk(
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        ),
    ]
}

fn ending_of(body: &str, headers: &str) -> (Vec<u8>, FinishedCall) {
    let upstream = piecewise(error_then_cut());
    let port = Arc::new(Port::default());
    let handle = gateway(&upstream, &port);
    let answer = call(&handle, body, headers);
    let finished = port.one();
    handle.shutdown();
    (answer, finished)
}

/// The byte relay: the client received the error event, then the answer was cut short.
#[test]
fn a_byte_relayed_stream_that_carried_its_wire_error_before_its_upstream_cut_is_a_wire_error() {
    let (answer, finished) = ending_of(r#"{"model":"code","stream":true}"#, "");
    assert!(answer.starts_with(b"HTTP/1.1 200 "), "{answer:?}");
    let text = String::from_utf8_lossy(&answer);
    assert!(text.contains("overloaded_error"), "{text}");
    assert_eq!(
        finished.ending,
        CallEnding::WireError,
        "the documented precedence puts WireError before UpstreamCut: {finished:#?}"
    );
}

/// The client-credential passthrough: the same answer, the same precedence.
#[test]
fn a_passed_through_stream_that_carried_its_wire_error_before_its_upstream_cut_is_a_wire_error() {
    let (answer, finished) = ending_of(
        r#"{"model":"sub","stream":true}"#,
        "x-api-key: client-key\r\nx-stainless-retry-count: 0\r\n",
    );
    assert!(answer.starts_with(b"HTTP/1.1 200 "), "{answer:?}");
    let text = String::from_utf8_lossy(&answer);
    assert!(text.contains("overloaded_error"), "{text}");
    assert_eq!(
        finished.ending,
        CallEnding::WireError,
        "the documented precedence puts WireError before UpstreamCut: {finished:#?}"
    );
}
