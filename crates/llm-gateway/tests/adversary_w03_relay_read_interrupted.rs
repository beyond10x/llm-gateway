//! Adversarial pass over `story:relay-read-interrupted`: an interrupted read of the target is
//! retried, and the story's acceptance says the reads' bounds still hold. The target's read is
//! bounded only by the connection's own read timeout (`RelayTarget::connect`: "its read and
//! write timeouts are the target's"), and a socket with `SO_RCVTIMEO` that is interrupted
//! restarts that whole timeout on the next `read` (`man 7 signal`). So a retry with no deadline
//! of its own waits for ever on a silent target while stops and continues keep arriving.
//!
//! The case stops and continues this test process from a `sh` child, deterministically, the way
//! the fix's own reproduction did, against a loopback listener that never answers. It is the
//! only case in this file, because the stops reach every thread of the test binary.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Child, Command},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-adversary-w03-interrupted-0123456789";
/// The target connection's read timeout: the only bound on a silent target.
const TARGET_READ_TIMEOUT: Duration = Duration::from_millis(400);
/// How long the client waits for the gateway's answer: ten target read timeouts.
const ANSWER_BOUND: Duration = Duration::from_secs(4);

struct Target {
    authority: String,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.authority)?;
        stream.set_read_timeout(Some(TARGET_READ_TIMEOUT))?;
        stream.set_write_timeout(Some(TARGET_READ_TIMEOUT))?;
        Ok(Box::new(stream))
    }
}

struct Source {
    authority: String,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target {
            authority: self.authority.clone(),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// Stops and continues this process every 25 ms until dropped. The stop and the continue are
/// apart, so the continue never discards a stop that is still pending.
struct Storm(Child);

impl Storm {
    fn start() -> Self {
        let pid = std::process::id();
        let script = format!(
            "while kill -STOP {pid} 2>/dev/null; do sleep 0.005; kill -CONT {pid}; sleep 0.02; done"
        );
        Self(Command::new("sh").arg("-c").arg(script).spawn().unwrap())
    }
}

impl Drop for Storm {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
        // A stop the killed loop sent last is never left standing.
        drop(
            Command::new("kill")
                .arg("-CONT")
                .arg(std::process::id().to_string())
                .status(),
        );
    }
}

struct Stop(Option<GatewayHandle>);

impl Drop for Stop {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.shutdown();
        }
    }
}

/// The client's whole exchange, its own reads retried when interrupted.
fn exchange(addr: std::net::SocketAddr, raw: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    stream.write_all(raw).unwrap();
    let mut received = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(0) | Err(_) => return received,
            Ok(read) => received.extend_from_slice(&chunk[..read]),
        }
    }
}

/// A silent target is answered `upstream-failed` within a bound of its read timeout while the
/// process is stopped and continued. Before the fix the first interrupted read answered 502 at
/// once; with the retry, every interruption restarts the target's read timeout and the request
/// is never answered while the stops go on.
#[test]
fn a_silent_target_under_stops_and_continues_is_still_answered_within_its_read_timeout() {
    // Accepts nothing and answers nothing: the connection completes in the backlog and the
    // request sits in the kernel's buffer.
    let silent = TcpListener::bind("127.0.0.1:0").unwrap();
    let source = Arc::new(Source {
        authority: silent.local_addr().unwrap().to_string(),
    });
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let model = RelayModel::new(
        Label::new("code").unwrap(),
        Label::new("served-code").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    let relay = Relay::new(vec![model], source as Arc<dyn RelayTargets>).unwrap();
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_secs(10);
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let addr = handle.local_addr();
    let _stop = Stop(Some(handle));

    let body = br#"{"model":"code","messages":[{"role":"user","content":"hi"}]}"#;
    let mut raw = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);

    let storm = Storm::start();
    let begun = Instant::now();
    let (sent, answered) = mpsc::channel();
    thread::spawn(move || drop(sent.send(exchange(addr, &raw))));
    let answer = answered.recv_timeout(ANSWER_BOUND);
    let waited = begun.elapsed();
    drop(storm);

    let answer = answer.unwrap_or_else(|_| {
        panic!(
            "no answer within {ANSWER_BOUND:?} ({waited:?} waited) from a target whose read \
             timeout is {TARGET_READ_TIMEOUT:?}: each interrupted read restarted it"
        )
    });
    let text = String::from_utf8_lossy(&answer);
    assert!(
        text.starts_with("HTTP/1.1 502 "),
        "a silent target is upstream-failed: {text}"
    );
    assert!(text.contains("\"code\":\"upstream-failed\""), "{text}");
}
