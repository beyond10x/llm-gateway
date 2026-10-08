//! Adversary cases, second pass, for story:gateway-deployment: the stop flag in `acquire`, and
//! `RunpodPool::invalidate` as the relay composition calls it. No pod is started, no Runpod API
//! is called and nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, load, runpod_pool, start_relaying};
use llm_runpod::{EmulatedRunpod, Identifier, ManualClock, PoolError};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, with_vllm_key};

const WAIT: Duration = Duration::from_secs(20);
const BODY: &str = "{\"model\":\"small\",\"messages\":[]}";

fn deployment(fixture: &Fixture, edit: impl Fn(String) -> String) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    load(&fixture.config(&edit(text))).unwrap()
}

/// A loopback pod answering every connection `200` with a fixed JSON body.
fn pod() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            let mut seen = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !seen.ends_with(BODY.as_bytes()) {
                match connection.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => seen.extend_from_slice(&chunk[..read]),
                }
            }
            let body = "{\"id\":\"chat-1\"}";
            drop(write!(
                connection,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            ));
        }
    });
    address
}

struct Loopback(SocketAddr);

impl PodConnector for Loopback {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

fn head(address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        BODY.len()
    )
    .unwrap();
    stream
}

fn finish(mut stream: TcpStream) -> String {
    stream.write_all(BODY.as_bytes()).unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// `docs/gateway.md`: `shutdown` "lets every accepted connection finish", and D2: "a request in
/// flight is answered". `docs/hosting.md` and `deployment.yaml` give up only on "a request still
/// waiting for its pod". `acquire` checks the stop flag before its first ask of the pool, so a
/// request accepted before the stop, whose pod is ready and serving, is answered
/// `target-unavailable` instead of being relayed, whenever its body is still arriving at the
/// signal.
#[test]
fn adv2_d2_a_request_accepted_before_the_stop_is_relayed_to_its_ready_pod() {
    let fixture = Fixture::new("adv-w03-2-inflight");
    let deployment = deployment(&fixture, |text| text);
    let running = start_relaying(
        &deployment,
        EmulatedRunpod::new(),
        Arc::new(Loopback(pod())),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    // The pod is started and ready: the first request is relayed.
    let warm = finish(head(address));
    assert!(warm.starts_with("HTTP/1.1 200 "), "{warm}");
    // The second request is accepted; its body is still on its way when the stop begins.
    let in_flight = head(address);
    thread::sleep(Duration::from_millis(300));
    let stopping = thread::spawn(move || running.shutdown());
    thread::sleep(Duration::from_millis(300));
    let answer = finish(in_flight);
    let report = stopping.join().unwrap();
    assert_eq!(report.accepted, report.completed);
    assert!(
        answer.starts_with("HTTP/1.1 200 "),
        "an accepted request to a ready pod was refused by the stop: {answer}"
    );
}

/// The stop answers a request still waiting for a cold pod `target-unavailable` (503), and
/// promptly.
#[test]
fn adv2_d2_a_request_waiting_for_its_pod_is_answered_target_unavailable_at_the_stop() {
    let fixture = Fixture::new("adv-w03-2-waiting");
    let deployment = deployment(&fixture, |text| text);
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod,
        Arc::new(Loopback(pod())),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let waiting = head(running.local_addr());
    let answer = thread::spawn(move || finish(waiting));
    thread::sleep(Duration::from_millis(800));
    running.shutdown();
    let answer = answer.join().unwrap();
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("target-unavailable"), "{answer}");
}

/// `RunpodPool::invalidate`: "a stale report never stops the replacement". The report for the
/// first pod, repeated after its replacement is up, leaves the replacement serving.
#[test]
fn adv2_w7_a_report_about_a_replaced_pod_never_stops_its_replacement() {
    let fixture = Fixture::new("adv-w03-2-stale");
    let deployment = deployment(&fixture, |text| text);
    let clock = ManualClock::new(1_000);
    let runpod = EmulatedRunpod::new();
    let pool = runpod_pool(&deployment, runpod.clone(), Arc::new(clock.clone())).unwrap();
    let small = Identifier::new("small").unwrap();
    let authorization = llm_gateway_cli::compute_authorization().unwrap();
    let ready = |clock: &ManualClock| {
        for _ in 0..10 {
            match pool.ensure(&small, &authorization) {
                Ok(lease) => return lease.endpoint().unwrap().to_owned(),
                Err(PoolError::Starting | PoolError::Stopping) => clock.advance(1_000),
                Err(other) => panic!("{}", other.code()),
            }
        }
        panic!("never ready");
    };
    let first = ready(&clock);
    // Two reports of one failure, as two requests through the pod would make.
    assert!(
        pool.invalidate(&small, |endpoint| endpoint == first)
            .unwrap()
    );
    assert!(
        !pool
            .invalidate(&small, |endpoint| endpoint == first)
            .unwrap()
    );
    let second = ready(&clock);
    assert_ne!(first, second);
    assert!(
        !pool
            .invalidate(&small, |endpoint| endpoint == first)
            .unwrap()
    );
    assert_eq!(ready(&clock), second);
    assert_eq!(runpod.create_calls(), 2);
}
