//! story:relay-read-interrupted, the pod's TLS connection: a read of a silent pod that is
//! interrupted again and again (this process stopped and continued every 25 ms) ends by the
//! connection's read timeout counted from the first attempt, never restarted by an
//! interruption, and an `Interrupted` never reaches the relay. One case in this file, because
//! the stops reach every thread of the test binary.

mod support;

use llm_gateway_cli::{PodConnector, ProxyConnector};
use std::{
    io::{self, Read},
    process::{Child, Command},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use support::tls_pod::{POD_HOST, TestRoot, TlsPod};

const READ_TIMEOUT: Duration = Duration::from_millis(600);
const ANSWER_BOUND: Duration = Duration::from_secs(5);

/// Stops and continues this process every 25 ms until dropped.
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
        drop(
            Command::new("kill")
                .arg("-CONT")
                .arg(std::process::id().to_string())
                .status(),
        );
    }
}

#[test]
fn tls_a_silent_pod_under_stops_and_continues_times_out_within_its_read_timeout() {
    let root = TestRoot::new("w03 interrupted test root");
    let pod = TlsPod::start(root.server(POD_HOST), None);
    let connector =
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(pod.address)).unwrap();
    // Opened before the storm: the handshake is not what is under test. The pod waits for a
    // request that never comes; `connect_probe` bounds the opening and each read by one timeout.
    let mut stream = connector.connect_probe(POD_HOST, READ_TIMEOUT).unwrap();

    let storm = Storm::start();
    let begun = Instant::now();
    let (sent, outcome) = mpsc::channel();
    thread::spawn(move || {
        let mut chunk = [0_u8; 64];
        let result = stream.read(&mut chunk);
        drop(sent.send(result));
    });
    let outcome = outcome.recv_timeout(ANSWER_BOUND);
    let waited = begun.elapsed();
    drop(storm);

    let result = outcome.unwrap_or_else(|_| {
        panic!("no end within {ANSWER_BOUND:?} ({waited:?}) of a {READ_TIMEOUT:?} read timeout")
    });
    let error = result.expect_err("a silent pod answers nothing");
    assert!(
        matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ),
        "an interrupted read is a timeout, never Interrupted: {error:?}"
    );
}
