//! story:live-runpod-wiring, criterion 1 past the create: the composition the shipped binary
//! runs (`start_connected`), over the production transport `ConnectorsRunpod` and a fixture of
//! the `connectors` CLI, with only the pod's address moved to a loopback pod.
//!
//! The binary reaches a pod at the Runpod proxy's `https://` endpoint, which it cannot open yet
//! (story:pod-proxy-tls), and no document key or option may point it elsewhere. So this test
//! wraps the transport in [`LoopbackProbe`], which hands the transport's own probe a loopback
//! `http://` endpoint and changes nothing else, and connects the relay to the loopback pod.
//! Everything else is the shipped composition: the binding from the document, each model's
//! vLLM key handed to the transport by alias, the reachability check, the pool and the relay.
//! Nothing leaves the loopback interface and no `connectors` but the fixture runs.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{PodConnector, load, start_connected};
use llm_runpod::{
    CreateAnswer, PodListing, PodRequest, Probe, ProbeTarget, RunpodTransport, TerminateAnswer,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use support::{
    Connectors, Fixture, OWNER_SECRET, VLLM_KEY, connected_document, created, listed, mutate,
    running_small_pod,
};

const WAIT: Duration = Duration::from_secs(10);
/// What the loopback pod answers every request with: a `GET /v1/models` listing that names the
/// model, which is also a body the relay passes through.
const POD_ANSWER: &str = r#"{"object":"list","data":[{"id":"small","object":"model"}]}"#;

/// The production transport with the probe's endpoint moved to a loopback pod.
struct LoopbackProbe<T> {
    inner: T,
    endpoint: String,
}

impl<T: RunpodTransport> RunpodTransport for LoopbackProbe<T> {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.inner.list_pods()
    }

    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.inner.create_pod(request)
    }

    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.inner.terminate_pod(pod_id)
    }

    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let endpoint = self.endpoint.clone();
        self.inner.probe_ready(&ProbeTarget {
            endpoint: Some(&endpoint),
            ..*target
        })
    }

    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.inner.container_started_at(pod_id)
    }
}

/// Reaches every pod at the loopback pod.
struct Loopback(SocketAddr);

impl PodConnector for Loopback {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// One request's head and `content-length` body.
fn read_request(connection: &mut TcpStream) -> Vec<u8> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        if let Some(end) = find(&raw, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
            let length: usize = head
                .split("\r\n")
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            if raw.len() >= end + 4 + length {
                return raw;
            }
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return raw,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    }
}

/// A loopback pod that answers every connection with [`POD_ANSWER`] and records each request.
fn loopback_pod() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::clone(&received);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            let request = read_request(&mut connection);
            recording
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&request).into_owned());
            let answer = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{POD_ANSWER}",
                POD_ANSWER.len()
            );
            drop(connection.write_all(answer.as_bytes()));
        }
    });
    (address, received)
}

fn post_chat(address: SocketAddr) -> String {
    let body = r#"{"model":"small","messages":[{"role":"user","content":"hi"}]}"#;
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn bearer(request: &str) -> Option<&str> {
    request.split("\r\n").find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then(|| value.trim().strip_prefix("Bearer "))
            .flatten()
    })
}

/// Criterion 1: a request for a cold model makes the connectors fixture receive one
/// `pod.create` with the declared GPU type tried first; the pod then receives the readiness
/// probe with the model's vLLM key, and after that the relayed request, with the same key.
#[test]
fn live_the_pod_receives_the_probe_with_the_vllm_key_and_then_the_relayed_request() {
    let fixture = Fixture::new("live-relay");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([running_small_pod()]))],
        "operations invoke pod.create": [created(&running_small_pod())],
    }));
    let deployment = load(&fixture.config(&connected_document(
        &fixture,
        "127.0.0.1:0",
        &connectors,
        25,
    )))
    .unwrap();
    let (pod, received) = loopback_pod();
    let endpoint = format!("http://{pod}/v1/");
    let relaying = start_connected(
        &deployment,
        move |transport| LoopbackProbe {
            inner: transport,
            endpoint,
        },
        Arc::new(Loopback(pod)),
    )
    .unwrap();
    // The startup sweep may probe the listed pod; nothing is relayed before a request.
    assert!(
        !received
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with("POST ")),
        "a request was relayed before any was made"
    );
    assert!(
        connectors
            .calls_of("operations invoke pod.create")
            .is_empty(),
        "a pod was created before any request"
    );

    let answer = post_chat(relaying.local_addr());
    relaying.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer:?}");
    assert!(answer.contains(POD_ANSWER), "{answer:?}");
    assert!(!answer.contains(VLLM_KEY), "{answer:?}");

    let creates = connectors.calls_of("operations invoke pod.create");
    assert_eq!(creates.len(), 1, "{creates:?}");
    let input: serde_json::Value =
        serde_json::from_str(creates[0]["input"].as_str().unwrap()).unwrap();
    assert_eq!(
        input["body"]["gpuTypeIds"],
        serde_json::json!(["NVIDIA L40S"])
    );

    let received = received.lock().unwrap().clone();
    let relayed = received
        .iter()
        .position(|request| request.starts_with("POST /v1/chat/completions "))
        .unwrap_or_else(|| panic!("the pod never received the relayed request: {received:?}"));
    let probes: Vec<&String> = received[..relayed]
        .iter()
        .filter(|request| request.starts_with("GET /v1/models "))
        .collect();
    assert!(
        !probes.is_empty(),
        "no readiness probe before the relayed request: {received:?}"
    );
    for probe in &probes {
        assert_eq!(bearer(probe), Some(VLLM_KEY), "{probe:?}");
    }
    assert_eq!(bearer(&received[relayed]), Some(VLLM_KEY), "{received:?}");
    assert!(
        !received[relayed].contains(OWNER_SECRET),
        "the owner token reached the pod: {received:?}"
    );
}

/// The library composition with the shipped transport and connector, from `text`.
fn start_shipped(text: &str, fixture: &Fixture) -> Result<llm_gateway_cli::Relaying, String> {
    let deployment = load(&fixture.config(text)).unwrap();
    start_connected(
        &deployment,
        |transport| transport,
        Arc::new(llm_gateway_cli::ProxyConnector),
    )
    .map_err(|refusal| refusal.to_string())
}

/// Criterion 3 at the library level: a connection that does not answer a complete `pods.list`
/// at start is unreachable. The composition starts, a request for the model is
/// `target-unavailable` (not `model-cold-start`), and the fixture receives nothing after the
/// start: no create, no approval, no further listing.
#[test]
fn live_an_unreachable_connection_starts_and_answers_target_unavailable() {
    let fixture = Fixture::new("live-unreachable");
    let connectors = Connectors::new(&fixture);
    // No script: every `operations invoke` fails without an answer.
    let relaying = start_shipped(
        &connected_document(&fixture, "127.0.0.1:0", &connectors, 5),
        &fixture,
    )
    .unwrap();
    let at_start = connectors.calls().len();
    assert_eq!(
        connectors.calls_of("operations invoke pods.list").len(),
        1,
        "the reachability check is one pods.list: {:?}",
        connectors.calls()
    );
    let answer = post_chat(relaying.local_addr());
    relaying.shutdown();
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer:?}");
    assert!(answer.contains("target-unavailable"), "{answer:?}");
    assert!(!answer.contains("model-cold-start"), "{answer:?}");
    assert_eq!(
        connectors.calls().len(),
        at_start,
        "the fixture received calls after the start: {:?}",
        connectors.calls()
    );
    assert!(
        connectors
            .calls_of("operations invoke pod.create")
            .is_empty()
    );
    assert!(!answer.contains(VLLM_KEY) && !answer.contains(OWNER_SECRET));
}

/// Criterion 3, the rest of "reachable": a missing executable and a work directory that is not
/// private are unreachable too, and the fixture receives nothing at all.
#[test]
fn live_a_missing_executable_or_a_shared_work_directory_is_unreachable() {
    for case in ["missing executable", "group-readable work directory"] {
        let fixture = Fixture::new(&format!("live-{}", case.replace(' ', "-")));
        let connectors = Connectors::new(&fixture);
        connectors.script(&serde_json::json!({
            "operations invoke pods.list": [listed(&serde_json::json!([]))],
        }));
        let mut text = connected_document(&fixture, "127.0.0.1:0", &connectors, 5);
        if case == "missing executable" {
            let missing = fixture.path("no-such-connectors");
            text = mutate(
                &text,
                &format!("executable = \"{}\"", connectors.executable().display()),
                &format!("executable = \"{}\"", missing.display()),
            );
        } else {
            std::fs::set_permissions(
                connectors.work(),
                std::os::unix::fs::PermissionsExt::from_mode(0o750),
            )
            .unwrap();
        }
        let relaying = start_shipped(&text, &fixture).unwrap();
        let answer = post_chat(relaying.local_addr());
        relaying.shutdown();
        assert!(answer.starts_with("HTTP/1.1 503 "), "{case}: {answer:?}");
        assert!(answer.contains("target-unavailable"), "{case}: {answer:?}");
        assert!(
            connectors.calls().is_empty(),
            "{case}: the fixture received {:?}",
            connectors.calls()
        );
    }
}

/// A connected model must name its vLLM key file: its pod always expects the key. Refused before
/// the connection is checked.
#[test]
fn live_a_connected_model_without_a_vllm_key_file_is_refused() {
    let fixture = Fixture::new("live-no-key");
    let connectors = Connectors::new(&fixture);
    let text = connected_document(&fixture, "127.0.0.1:0", &connectors, 5);
    let key = fixture.path("vllm-key");
    let text = mutate(
        &text,
        &format!("vllm_api_key_file = \"{}\"\n", key.display()),
        "",
    );
    let refused = start_shipped(&text, &fixture).err().unwrap();
    assert!(
        refused.starts_with("config:value: models.small.vllm_api_key_file is required"),
        "{refused}"
    );
    assert!(connectors.calls().is_empty(), "{:?}", connectors.calls());
}

/// The shipped `start` refuses what `start_connected` composes: the wiring is inert in the
/// binary until story:pod-proxy-tls, and the refusal comes before any connectors call.
#[test]
fn live_start_refuses_a_connectors_document_before_any_call() {
    let fixture = Fixture::new("live-start-inert");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([]))],
    }));
    let deployment =
        load(&fixture.config(&connected_document(&fixture, "127.0.0.1:0", &connectors, 5)))
            .unwrap();
    let refused = llm_gateway_cli::start(&deployment).err().unwrap();
    assert_eq!(refused.code(), "config:value", "{refused}");
    assert!(connectors.calls().is_empty(), "{:?}", connectors.calls());
}
