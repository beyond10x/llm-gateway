//! Adversarial pass 1 over `story:pod-proxy-tls`: the pod connection's certificate check, the
//! readiness probe's path to the pod, the shipped binary's reach to the test seam, and the TLS
//! crates' place in the workspace (spec/domains/deployment.yaml, the pod endpoint rule and the
//! TLS client and its trust roots).
//!
//! Every pod here is a loopback fixture; every certificate is made at run time by `rcgen`; no
//! host certificate store is read, nothing leaves the loopback interface, and no `connectors`
//! but the fixture runs.

mod support;

use llm_gateway_cli::{PodConnector, ProxyConnector, load, start, start_connected};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, date_time_ymd,
};
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use support::{
    Connectors, Fixture, OWNER_SECRET, VLLM_KEY, connected_document, created, listed, mutate,
    running_small_pod,
    tls_pod::{POD_HOST, Seen, TlsPod},
};

const SETTLE: Duration = Duration::from_secs(10);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A certificate authority made for one test, which can issue any leaf.
struct Ca {
    der: Vec<u8>,
    issuer: Issuer<'static, KeyPair>,
}

impl Ca {
    fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let key = KeyPair::generate().unwrap();
        let certificate = params.self_signed(&key).unwrap();
        Self {
            der: certificate.der().to_vec(),
            issuer: Issuer::new(params, key),
        }
    }
}

/// Leaf parameters for `names`, a server-auth certificate valid now.
fn leaf(names: &[&str], common_name: &str) -> CertificateParams {
    let mut params = CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params
}

/// A server presenting `chain` (leaf first) for `key`.
fn server(chain: Vec<CertificateDer<'static>>, key: &KeyPair) -> Arc<ServerConfig> {
    Arc::new(
        ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                chain,
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .unwrap(),
    )
}

/// A server presenting the leaf `params` signed by `ca`.
fn issued(ca: &Ca, params: &CertificateParams) -> Arc<ServerConfig> {
    let key = KeyPair::generate().unwrap();
    let certificate = params.signed_by(&key, &ca.issuer).unwrap();
    server(vec![certificate.der().clone()], &key)
}

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

/// Each certificate the pod endpoint rule refuses, presented by a pod the seam trusts the root
/// of: the relay's `connect` and the probe's `connect_probe` both fail, and the pod completes no
/// handshake and decrypts no byte.
fn assert_refused(case: &str, trusted: &Ca, config: &Arc<ServerConfig>) {
    for probe in [false, true] {
        let pod = TlsPod::start(Arc::clone(config), None);
        let connector =
            ProxyConnector::with_test_seam(std::slice::from_ref(&trusted.der), Some(pod.address))
                .unwrap();
        let opened = if probe {
            connector
                .connect_probe(POD_HOST, Duration::from_secs(5))
                .is_ok()
        } else {
            connector.connect(POD_HOST).is_ok()
        };
        assert!(!opened, "{case} (probe: {probe}): the connection opened");
        let seen = seen_at_least(&pod, 1);
        assert_eq!(seen.len(), 1, "{case} (probe: {probe}): {seen:?}");
        assert!(!seen[0].handshake, "{case} (probe: {probe}): {seen:?}");
        assert!(
            seen[0].request.is_empty(),
            "{case} (probe: {probe}): {seen:?}"
        );
    }
}

/// The certificates a pod endpoint might present that the pod endpoint rule must refuse, each
/// for the pod's own host name unless the case says otherwise, each beside the root the seam
/// trusts: expired, not yet valid, self-signed, issued by a leaf that is no CA, naming the host
/// only in its common name, a wildcard one label too wide, and one for client authentication
/// only. The controls are accepted: the exact name and the proxy's own wildcard.
#[test]
fn adversary_w07_every_bad_certificate_fails_both_connections_and_the_pod_reads_nothing() {
    let trusted = Ca::new("adversary-w07 trusted root");

    let mut expired = leaf(&[POD_HOST], POD_HOST);
    expired.not_before = date_time_ymd(1999, 1, 1);
    expired.not_after = date_time_ymd(2001, 1, 1);
    assert_refused("expired", &trusted, &issued(&trusted, &expired));

    let mut early = leaf(&[POD_HOST], POD_HOST);
    early.not_before = date_time_ymd(2090, 1, 1);
    early.not_after = date_time_ymd(2091, 1, 1);
    assert_refused("not yet valid", &trusted, &issued(&trusted, &early));

    let key = KeyPair::generate().unwrap();
    let own = leaf(&[POD_HOST], POD_HOST).self_signed(&key).unwrap();
    assert_refused(
        "self-signed",
        &trusted,
        &server(vec![own.der().clone()], &key),
    );

    // A leaf of the trusted root, for another host and no CA, signing one for the pod's host.
    let middle_params = leaf(&["middle.example"], "middle.example");
    let middle_key = KeyPair::generate().unwrap();
    let middle = middle_params
        .signed_by(&middle_key, &trusted.issuer)
        .unwrap();
    let middle_issuer = Issuer::new(middle_params, middle_key);
    let key = KeyPair::generate().unwrap();
    let below = leaf(&[POD_HOST], POD_HOST)
        .signed_by(&key, &middle_issuer)
        .unwrap();
    assert_refused(
        "issued by a leaf",
        &trusted,
        &server(vec![below.der().clone(), middle.der().clone()], &key),
    );

    assert_refused(
        "the host in the common name only",
        &trusted,
        &issued(&trusted, &leaf(&["elsewhere.example"], POD_HOST)),
    );
    assert_refused(
        "a wildcard one label too wide",
        &trusted,
        &issued(&trusted, &leaf(&["*.runpod.net"], "*.runpod.net")),
    );

    let mut client_only = leaf(&[POD_HOST], POD_HOST);
    client_only.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    assert_refused(
        "client authentication only",
        &trusted,
        &issued(&trusted, &client_only),
    );

    // The controls: the exact host, and the proxy's own wildcard, are accepted.
    for names in [[POD_HOST], ["*.proxy.runpod.net"]] {
        let pod = TlsPod::start(issued(&trusted, &leaf(&names, names[0])), None);
        let connector =
            ProxyConnector::with_test_seam(std::slice::from_ref(&trusted.der), Some(pod.address))
                .unwrap();
        let mut stream = connector.connect(POD_HOST).unwrap();
        write!(
            stream,
            "GET /v1/models HTTP/1.1\r\nhost: {POD_HOST}\r\nconnection: close\r\n\r\n"
        )
        .unwrap();
        stream.flush().unwrap();
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        assert!(
            answer.starts_with(b"HTTP/1.1 200 "),
            "{names:?}: {answer:?}"
        );
    }
}

/// A plain-TCP endpoint on the pod's port, and every byte it received.
struct Plain {
    address: SocketAddr,
    received: Arc<Mutex<Vec<u8>>>,
}

/// A listener that answers in plain HTTP: at once, after the client's first bytes, or never
/// (it reads and stays silent), or by closing.
#[derive(Clone, Copy, Debug)]
enum Answer {
    AtOnce,
    AfterHello,
    Close,
}

impl Plain {
    fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&received);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let recording = Arc::clone(&recording);
                thread::spawn(move || {
                    connection
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let http = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
                    let mut buffer = [0_u8; 8192];
                    match answer {
                        Answer::AtOnce => drop(connection.write_all(http)),
                        Answer::Close => return,
                        Answer::AfterHello => {}
                    }
                    let mut first = true;
                    while let Ok(read) = connection.read(&mut buffer) {
                        if read == 0 {
                            break;
                        }
                        recording.lock().unwrap().extend_from_slice(&buffer[..read]);
                        if first && matches!(answer, Answer::AfterHello) {
                            drop(connection.write_all(http));
                        }
                        first = false;
                    }
                });
            }
        });
        Self { address, received }
    }

    fn received(&self) -> Vec<u8> {
        self.received.lock().unwrap().clone()
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// A plain-HTTP answer on the `https` port is no connection, for the relay and for the probe,
/// whether it is written at once, after the client hello or never; nothing the client sent
/// holds a request line or a key.
#[test]
fn adversary_w07_a_plain_http_answer_on_the_https_port_opens_no_connection() {
    let root = Ca::new("adversary-w07 plain root");
    for answer in [Answer::AtOnce, Answer::AfterHello, Answer::Close] {
        for probe in [false, true] {
            let plain = Plain::start(answer);
            let connector = ProxyConnector::with_test_seam(
                std::slice::from_ref(&root.der),
                Some(plain.address),
            )
            .unwrap();
            let opened = if probe {
                connector
                    .connect_probe(POD_HOST, Duration::from_secs(5))
                    .is_ok()
            } else {
                connector.connect(POD_HOST).is_ok()
            };
            assert!(
                !opened,
                "{answer:?} (probe: {probe}): the connection opened"
            );
            thread::sleep(Duration::from_millis(100));
            let received = plain.received();
            for needle in [
                &b"GET "[..],
                b"POST ",
                b"authorization",
                VLLM_KEY.as_bytes(),
            ] {
                assert!(
                    !contains(&received, needle),
                    "{answer:?} (probe: {probe}): the endpoint received {:?}",
                    String::from_utf8_lossy(&received)
                );
            }
        }
    }
}

// --- The readiness probe through the composition ----------------------------------------------

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

/// The probe path, composed as the shipped `start` composes it (`start_connected`, the transport
/// unwrapped) with the seam's connector: the pod presents a certificate for its own host from a
/// root the connector does not trust from the first connection on. Every probe fails before a
/// byte: the pod completes no handshake and decrypts nothing, so it never sees the vLLM key, the
/// pod never becomes ready, and the client is not answered from it.
#[test]
fn adversary_w07_an_untrusted_pod_is_probed_without_a_byte_or_a_key() {
    let fixture = Fixture::new("adversary-w07-probe-untrusted");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([running_small_pod()]))],
        "operations invoke pod.create": [created(&running_small_pod())],
    }));
    let deployment =
        load(&fixture.config(&connected_document(&fixture, "127.0.0.1:0", &connectors, 3)))
            .unwrap();
    let trusted = Ca::new("adversary-w07 trusted root");
    let stranger = Ca::new("adversary-w07 stranger root");
    let pod = TlsPod::start(issued(&stranger, &leaf(&[POD_HOST], POD_HOST)), None);
    let connector =
        ProxyConnector::with_test_seam(std::slice::from_ref(&trusted.der), Some(pod.address))
            .unwrap();
    let relaying = start_connected(&deployment, |transport| transport, Arc::new(connector))
        .unwrap_or_else(|refusal| panic!("refused: {refusal}"));
    let answer = post_chat(relaying.local_addr());
    relaying.shutdown();

    assert!(!answer.starts_with("HTTP/1.1 200 "), "{answer:?}");
    assert!(!answer.contains(VLLM_KEY), "{answer:?}");
    let seen = pod.seen();
    assert!(
        !seen.is_empty(),
        "the probe never reached the pod, so the case proves nothing"
    );
    for connection in &seen {
        assert!(!connection.handshake, "{connection:?}");
        assert!(connection.request.is_empty(), "{connection:?}");
    }
}

// --- The shipped `start` with a `connectors` document -----------------------------------------

/// With `inert_until_tls` gone, the shipped `start` composes a `connectors` document. Every
/// refusal the document itself earns still comes before any `connectors` call, and none quotes
/// a key: a vLLM key file that is not a token, one that is missing, an owner secret that is
/// missing, and a model the pool cannot run.
#[test]
fn adversary_w07_the_shipped_start_refuses_a_bad_connectors_document_before_any_call() {
    let fixture = Fixture::new("adversary-w07-start-refusals");
    let connectors = Connectors::new(&fixture);
    connectors.script(&serde_json::json!({
        "operations invoke pods.list": [listed(&serde_json::json!([]))],
    }));
    let base = connected_document(&fixture, "127.0.0.1:0", &connectors, 1);
    let key_path = fixture.vllm_key().display().to_string();
    let secret_path = fixture.owner_secret().display().to_string();
    let not_a_token = "not a token small-model-key-with-spaces";
    let bad_key = fixture.write("bad-key", format!("{not_a_token}\n").as_bytes(), 0o600);
    let cases = [
        (
            "a key that is not a token",
            mutate(&base, &key_path, &bad_key.display().to_string()),
        ),
        (
            "a missing key file",
            mutate(
                &base,
                &key_path,
                &fixture.path("no-such-key").display().to_string(),
            ),
        ),
        (
            "a missing owner secret",
            mutate(
                &base,
                &secret_path,
                &fixture.path("no-such-secret").display().to_string(),
            ),
        ),
        (
            "a model the pool cannot run",
            mutate(
                &base,
                "image = \"vllm/vllm-openai:v0.27.1\"",
                "image = \"vllm/vllm-openai v0.27.1\"",
            ),
        ),
    ];
    for (case, text) in cases {
        let refusal = match load(&fixture.config(&text)) {
            Err(refusal) => refusal,
            Ok(deployment) => match start(&deployment) {
                Ok(relaying) => {
                    relaying.shutdown();
                    panic!("{case}: the shipped start served it");
                }
                Err(refusal) => refusal,
            },
        };
        let text = refusal.to_string();
        for secret in [VLLM_KEY, OWNER_SECRET, not_a_token] {
            assert!(!text.contains(secret), "{case}: {text}");
        }
        assert!(
            connectors.calls().is_empty(),
            "{case}: the fixture received {:?}",
            connectors.calls()
        );
    }
}

// --- The shipped binary and the seam ----------------------------------------------------------

fn bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

/// What `main` reaches is what the linker keeps. The shipped binary holds neither seam variable
/// nor the seam constructor's own refusal text, nor a variable a native certificate store would
/// read; `gateway-tls-seam`, built from the same library, holds the first two (the control
/// that the search is not vacuous), and both hold a line `main_with` writes.
#[test]
fn adversary_w07_the_shipped_binary_links_no_seam_and_reads_no_certificate_variable() {
    let shipped = bytes(env!("CARGO_BIN_EXE_b10x-llm-gateway"));
    let seam = bytes(env!("CARGO_BIN_EXE_gateway-tls-seam"));
    let seam_only: [&[u8]; 3] = [
        b"GATEWAY_TLS_SEAM_ROOT",
        b"GATEWAY_TLS_SEAM_POD",
        b"the test seam maps pod hosts to a loopback address only",
    ];
    for needle in seam_only {
        assert!(
            contains(&seam, needle),
            "control: the seam lacks {:?}",
            String::from_utf8_lossy(needle)
        );
        assert!(
            !contains(&shipped, needle),
            "the shipped binary holds {:?}",
            String::from_utf8_lossy(needle)
        );
    }
    for needle in [&b"SSL_CERT_FILE"[..], b"SSL_CERT_DIR"] {
        assert!(
            !contains(&shipped, needle),
            "the shipped binary holds {:?}",
            String::from_utf8_lossy(needle)
        );
    }
    for binary in [&shipped, &seam] {
        assert!(contains(binary, b"listening on "), "control: no main_with");
    }
}

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cargo(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cargo {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// Criterion 4 says no TLS crate enters the gateway crate's closure, and the spec says
/// `b10x-llm-runpod` links none either; `dependency_boundary.rs` forbids neither `rustls` nor
/// `ring` nor `webpki`, so it would pass if one did. This reads both closures from `cargo tree`,
/// with the CLI crate's as the control that the search finds them.
#[test]
fn adversary_w07_no_tls_crate_in_the_gateway_or_runpod_closure() {
    let tls = [
        "rustls",
        "ring",
        "webpki",
        "openssl",
        "native-tls",
        "aws-lc",
    ];
    let closure = |package: &str| {
        cargo(&[
            "tree",
            "--offline",
            "--locked",
            "-p",
            package,
            "-e",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
    };
    let names = |tree: &str| -> Vec<String> {
        tree.lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect()
    };
    let control = names(&closure("b10x-llm-gateway-cli"));
    assert!(
        control.iter().any(|name| name == "rustls"),
        "control: {control:?}"
    );
    for package in ["b10x-llm-gateway", "b10x-llm-runpod"] {
        let found: Vec<String> = names(&closure(package))
            .into_iter()
            .filter(|name| tls.iter().any(|tls| name.starts_with(tls)))
            .collect();
        assert!(found.is_empty(), "{package} links {found:?}");
    }
}
