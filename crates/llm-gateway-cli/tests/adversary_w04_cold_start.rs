//! Adversary cases for story:cold-start-hold: a pod that fails while requests are held for it,
//! and `retry-after` on every wire. Composed through `start_relaying` with `EmulatedRunpod`, a
//! manual pool clock and a loopback pod. No pod is started, no Runpod API is called and nothing
//! leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, load, start_relaying};
use llm_runpod::{EmulatedRunpod, ManualClock, PodStatus};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread,
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, mutate, with_vllm_key};

const WAIT: Duration = Duration::from_secs(20);
const CHAT: &str = "/v1/chat/completions";
const SMALL_CHAT: &str = "{\"model\":\"small\",\"messages\":[]}";

/// The support document on an ephemeral loopback port, serving every wire, with `lines` added
/// to `[models.small]`.
fn deployment(fixture: &Fixture, lines: &str) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let text = mutate(
        &text,
        "wires = [\"chat\", \"responses\"]",
        "wires = [\"chat\", \"responses\", \"messages\"]",
    );
    let text = mutate(
        &text,
        "max_model_len = 1024\n",
        &format!("max_model_len = 1024\n{lines}"),
    );
    load(&fixture.config(&text)).unwrap()
}

/// A loopback address nothing answers on: no case here reaches a pod.
struct Nowhere(SocketAddr);

impl PodConnector for Nowhere {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

fn nowhere() -> Arc<dyn PodConnector> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    Arc::new(Nowhere(address))
}

fn post(address: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn header<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.split("\r\n\r\n")
        .next()?
        .split("\r\n")
        .skip(1)
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
        })
}

/// `deployment.yaml` (row W6): `model-cold-start` is for a model "still starting" when the hold
/// passes; "any other pool refusal is `target-unavailable`". Two requests are held together on
/// one pod (`l6_requests_held_together_wait_on_one_pod`), and that pod misses its startup
/// deadline while both are held: it failed, for both of them. The pool reports the failure only
/// to whichever request's step retired the pod; the other is told `stopping`, which the relay
/// treats as still starting, so it starts a second pod, holds on, and is told to retry in 30 s
/// for a model whose pod just failed.
#[test]
fn adv_w6_every_request_held_on_a_pod_that_misses_its_startup_deadline_is_target_unavailable() {
    let fixture = Fixture::new("adv-w04-deadline-together");
    let deployment = deployment(
        &fixture,
        "start_wait_seconds = 10\nrequest_hold_seconds = 4\n",
    );
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let clock = ManualClock::new(1_000);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        nowhere(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    let address = running.local_addr();
    let first = thread::spawn(move || post(address, CHAT, SMALL_CHAT));
    thread::sleep(Duration::from_millis(200));
    let second = thread::spawn(move || post(address, CHAT, SMALL_CHAT));
    thread::sleep(Duration::from_millis(600));
    // Both are held on pod1. Its 10 s startup deadline passes on the pool's clock.
    assert_eq!(runpod.create_calls(), 1);
    clock.advance(11_000);
    let answers = [first.join().unwrap(), second.join().unwrap()];
    running.shutdown();
    let creates = runpod.create_calls();
    for answer in &answers {
        assert!(
            answer.contains("\"code\":\"target-unavailable\""),
            "a request held while its pod missed its startup deadline was answered (creates: {creates}): {answer}"
        );
        assert_eq!(header(answer, "retry-after"), None, "{answer}");
    }
}

/// The same failure seen by one held request alone: the pod exits during the hold. That request
/// observes the pool's own refusal and is `target-unavailable`, without `retry-after`.
#[test]
fn adv_w6_a_request_held_on_a_pod_that_exits_is_target_unavailable() {
    let fixture = Fixture::new("adv-w04-exited");
    let deployment = deployment(&fixture, "request_hold_seconds = 5\n");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        nowhere(),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let held = thread::spawn(move || post(address, CHAT, SMALL_CHAT));
    thread::sleep(Duration::from_millis(700));
    runpod.set_status("pod1", PodStatus::Exited);
    let answer = held.join().unwrap();
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(
        answer.contains("\"code\":\"target-unavailable\""),
        "{answer}"
    );
    assert_eq!(header(&answer, "retry-after"), None, "{answer}");
}

/// `docs/gateway.md`: `model-cold-start` carries `retry-after: 30`. The W6 tests measure only
/// the chat wire; the responses and messages wires answer the same refusal with the same header.
#[test]
fn adv_w6_every_wire_answers_model_cold_start_with_retry_after_30() {
    let fixture = Fixture::new("adv-w04-wires");
    let deployment = deployment(&fixture, "request_hold_seconds = 0\n");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        nowhere(),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let answers: Vec<String> = [
        (CHAT, SMALL_CHAT),
        ("/v1/responses", "{\"model\":\"small\",\"input\":\"hi\"}"),
        (
            "/v1/messages",
            "{\"model\":\"small\",\"max_tokens\":8,\"messages\":[]}",
        ),
    ]
    .iter()
    .map(|(path, body)| post(address, path, body))
    .collect();
    running.shutdown();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert!(answer.contains("\"code\":\"model-cold-start\""), "{answer}");
        assert_eq!(header(answer, "retry-after"), Some("30"), "{answer}");
    }
    assert_eq!(runpod.create_calls(), 1);
}
