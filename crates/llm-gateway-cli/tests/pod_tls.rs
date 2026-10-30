//! story:pod-proxy-tls: the binary reaches a pod at its `https` proxy URL over TLS, with the
//! certificate verified before any request byte is written (spec/domains/deployment.yaml, the
//! pod endpoint rule).
//!
//! The connector is driven at the library level against loopback `https` fixture pods
//! (`support::tls_pod`), and the binary end to end through `gateway-tls-seam`, the shipped `main`
//! with the test seam: a test root, and the pod's proxy host mapped to the fixture pod's
//! loopback address while the certificate is still verified for that host. Its Runpod side is
//! the `connectors` fixture. Nothing leaves the loopback interface, no host certificate store is
//! read, and no `connectors` but the fixture runs.

mod support;

use llm_gateway_cli::{PodConnector, ProxyConnector};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};
use support::{
    Connectors, Fixture, OWNER_SECRET, VLLM_KEY, connected_document, created, listed,
    running_small_pod,
    tls_pod::{POD_CHAT_ANSWER, POD_HOST, Seen, Switch, TestRoot, TlsPod},
};

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(10);

/// Waits until the fixture pod has recorded `count` connections, and returns them.
fn seen_at_least(pod: &TlsPod, count: usize) -> Vec<Seen> {
    let deadline = Instant::now() + SETTLE;
    loop {
        let seen = pod.seen();
        if seen.len() >= count || Instant::now() > deadline {
            return seen;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.split("\r\n").skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

// --- The connector, at the library level ------------------------------------------------------

/// A certificate that chains to the seam's root and names the pod's host: the connection opens,
/// the client asked for the pod's host name (SNI), and a request and its answer pass over it.
#[test]
fn tls_the_connector_opens_a_verified_connection_to_the_pod_host() {
    let root = TestRoot::new("pod-tls test root");
    let pod = TlsPod::start(root.server(POD_HOST), None);
    let connector =
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(pod.address)).unwrap();
    let mut stream = connector.connect(POD_HOST).unwrap();
    write!(
        stream,
        "GET /v1/models HTTP/1.1\r\nhost: {POD_HOST}\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    stream.flush().unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    let answer = String::from_utf8(answer).unwrap();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer:?}");
    assert!(answer.contains("\"small\""), "{answer:?}");
    let seen = seen_at_least(&pod, 1);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].handshake, "{seen:?}");
    assert_eq!(seen[0].server_name.as_deref(), Some(POD_HOST));
    assert!(seen[0].text().starts_with("GET /v1/models "), "{seen:?}");
}

/// Criterion 2 at the connector: a certificate from a root the client does not trust, and one
/// from the trusted root for another host, each fail `connect` itself, and the fixture pod
/// decrypts zero request bytes. The control is the case above.
#[test]
fn tls_an_untrusted_certificate_fails_the_connect_and_the_pod_reads_nothing() {
    let trusted = TestRoot::new("pod-tls trusted root");
    let stranger = TestRoot::new("pod-tls untrusted root");
    for (case, config) in [
        ("an untrusted root", stranger.server(POD_HOST)),
        (
            "another host's certificate",
            trusted.server("podother1-8000.proxy.runpod.net"),
        ),
    ] {
        let pod = TlsPod::start(config, None);
        let connector =
            ProxyConnector::with_test_seam(std::slice::from_ref(&trusted.der), Some(pod.address))
                .unwrap();
        let error = connector.connect(POD_HOST).err();
        assert!(error.is_some(), "{case}: the connection opened");
        let seen = seen_at_least(&pod, 1);
        assert_eq!(seen.len(), 1, "{case}: {seen:?}");
        assert!(!seen[0].handshake, "{case}: {seen:?}");
        assert!(seen[0].request.is_empty(), "{case}: {seen:?}");
    }
}

/// The shipped connector trusts only the roots compiled into the binary: a certificate a test
/// root issued is refused, and the pod reads nothing. `localhost` resolves without a network.
#[test]
fn tls_the_shipped_connector_does_not_trust_a_test_root() {
    let root = TestRoot::new("pod-tls test root");
    let pod = TlsPod::start(root.server("localhost"), None);
    let authority = format!("localhost:{}", pod.address.port());
    let error = ProxyConnector::shipped().connect(&authority).err();
    assert!(error.is_some(), "the shipped connector trusted a test root");
    let seen = seen_at_least(&pod, 1);
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(!seen[0].handshake, "{seen:?}");
    assert!(seen[0].request.is_empty(), "{seen:?}");
    // The control: the same pod through the seam, trusting that root.
    let seam = ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), None).unwrap();
    let mut stream = seam.connect(&authority).unwrap();
    stream
        .write_all(b"GET /v1/models HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    assert!(answer.starts_with(b"HTTP/1.1 200 "), "{answer:?}");
}

/// The seam maps pod hosts to a loopback address only, and takes no root it cannot read.
#[test]
fn tls_the_test_seam_takes_only_loopback_and_readable_roots() {
    let root = TestRoot::new("pod-tls test root");
    let elsewhere: SocketAddr = "192.0.2.1:443".parse().unwrap();
    assert!(
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(elsewhere)).is_err()
    );
    assert!(ProxyConnector::with_test_seam(&[], None).is_err());
    assert!(ProxyConnector::with_test_seam(&[b"not a certificate".to_vec()], None).is_err());
    let loopback: SocketAddr = "127.0.0.1:443".parse().unwrap();
    assert!(
        ProxyConnector::with_test_seam(std::slice::from_ref(&root.der), Some(loopback)).is_ok()
    );
}

// --- The binary, end to end -------------------------------------------------------------------

/// `gateway-tls-seam`: the shipped `main` with the seam's root and loopback pod.
struct SeamServer {
    child: Child,
    address: SocketAddr,
    lines: Receiver<String>,
}

impl SeamServer {
    fn start(config: &Path, root: &Path, pod: SocketAddr) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gateway-tls-seam"))
            .args(["--config", config.to_str().unwrap()])
            .env("GATEWAY_TLS_SEAM_ROOT", root)
            .env("GATEWAY_TLS_SEAM_POD", pod.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + STARTUP;
        let mut seen = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                drop(child.wait());
                panic!("gateway-tls-seam never reported {LISTENING:?}; stderr: {seen:?}");
            };
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
            seen.push(line);
        };
        Self {
            child,
            address,
            lines,
        }
    }

    /// Stops the process and returns everything it wrote to standard error after it listened.
    fn stop(mut self) -> Vec<String> {
        drop(self.child.kill());
        drop(self.child.wait());
        let mut rest = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(Duration::from_secs(5)) {
            rest.push(line);
        }
        rest
    }
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

/// Criteria 1 and 6: the binary, with a `connectors` provider against the connectors fixture,
/// starts the model's pod and relays a chat request to it at the address llm's Runpod
/// description gives, `https://podsmall1-8000.proxy.runpod.net/v1/`, over TLS verified against
/// the seam's test root. The pod receives the readiness probe and then the relayed request,
/// both with the model's vLLM key and both for its own host name, and the client receives the
/// pod's answer.
#[test]
fn tls_the_binary_relays_a_chat_request_to_the_https_pod_through_connectors() {
    let fixture = Fixture::new("tls-relay");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "invoke pods.list": [listed(&serde_json::json!([running_small_pod()]))],
        "invoke pod.create": [created(&running_small_pod())],
    }));
    let config = fixture.config(&connected_document(
        &fixture,
        "127.0.0.1:0",
        &connectors,
        25,
    ));
    let root = TestRoot::new("pod-tls test root");
    let root_file = fixture.write("test-root.der", &root.der, 0o600);
    let pod = TlsPod::start(root.server(POD_HOST), None);
    let server = SeamServer::start(&config, &root_file, pod.address);
    let answer = post_chat(server.address);
    let stderr = server.stop();

    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer:?}");
    assert!(answer.contains(POD_CHAT_ANSWER), "{answer:?}");
    assert!(!answer.contains(VLLM_KEY), "{answer:?}");
    let creates = connectors.calls_of("invoke pod.create");
    assert_eq!(creates.len(), 1, "{creates:?}");

    let seen = pod.seen();
    let relayed = seen
        .iter()
        .find(|connection| connection.text().starts_with("POST /v1/chat/completions "))
        .unwrap_or_else(|| panic!("the pod never received the relayed request: {seen:?}"));
    let probes: Vec<&Seen> = seen
        .iter()
        .filter(|connection| connection.text().starts_with("GET /v1/models "))
        .collect();
    assert!(
        !probes.is_empty(),
        "no readiness probe reached the pod: {seen:?}"
    );
    for connection in probes.iter().copied().chain([relayed]) {
        assert!(connection.handshake, "{connection:?}");
        assert_eq!(connection.server_name.as_deref(), Some(POD_HOST));
        let request = connection.text();
        assert_eq!(header(&request, "host"), Some(POD_HOST), "{request:?}");
        assert_eq!(
            header(&request, "authorization"),
            Some(format!("Bearer {VLLM_KEY}").as_str()),
            "{request:?}"
        );
        assert!(!request.contains(OWNER_SECRET), "{request:?}");
    }
    for line in &stderr {
        assert!(
            !line.contains(VLLM_KEY) && !line.contains(OWNER_SECRET),
            "{line}"
        );
    }
}

/// Criterion 2: once the pod's certificate is one the binary does not trust, the relayed
/// request is answered `upstream-failed` and the pod decrypts zero request bytes. The pod
/// serves the readiness probe with the trusted certificate, so that the pod is ready and the
/// relay is what connects next; from the first probe answered after the `pod.create`, it
/// presents a certificate for the same host from an untrusted root.
#[test]
fn tls_an_untrusted_pod_certificate_is_upstream_failed_and_the_pod_reads_nothing() {
    let fixture = Fixture::new("tls-untrusted");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "invoke pods.list": [listed(&serde_json::json!([running_small_pod()]))],
        "invoke pod.create": [created(&running_small_pod())],
    }));
    let config = fixture.config(&connected_document(
        &fixture,
        "127.0.0.1:0",
        &connectors,
        25,
    ));
    let root = TestRoot::new("pod-tls test root");
    let stranger = TestRoot::new("pod-tls untrusted root");
    let root_file = fixture.write("test-root.der", &root.der, 0o600);
    let calls = connectors.dir.join("calls.jsonl");
    let pod = TlsPod::start(
        root.server(POD_HOST),
        Some(Switch {
            to: stranger.server(POD_HOST),
            armed: Box::new(move || {
                std::fs::read_to_string(&calls)
                    .is_ok_and(|calls| calls.contains("\"invoke pod.create\""))
            }),
        }),
    );
    let server = SeamServer::start(&config, &root_file, pod.address);
    let answer = post_chat(server.address);
    let stderr = server.stop();

    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer:?}");
    assert!(answer.contains("upstream-failed"), "{answer:?}");
    let seen = pod.seen();
    assert!(
        seen.iter()
            .any(|connection| connection.handshake
                && connection.text().starts_with("GET /v1/models ")),
        "the pod was never probed ready: {seen:?}"
    );
    let untrusted: Vec<&Seen> = seen
        .iter()
        .filter(|connection| connection.switched)
        .collect();
    assert!(
        !untrusted.is_empty(),
        "the relay never connected to the untrusted pod: {seen:?}"
    );
    for connection in &untrusted {
        assert!(!connection.handshake, "{connection:?}");
        assert!(connection.request.is_empty(), "{connection:?}");
    }
    assert!(
        !seen
            .iter()
            .any(|connection| connection.text().starts_with("POST ")),
        "a request reached the pod: {seen:?}"
    );
    for line in &stderr {
        assert!(
            !line.contains(VLLM_KEY) && !line.contains(OWNER_SECRET),
            "{line}"
        );
    }
}
