//! The binary's relay to Runpod pods, composed in process through the `start_relaying` seam with
//! `EmulatedRunpod` and a loopback pod (row B8), and the pool's inputs from the deployment
//! document, `idle_timeout_minutes = 0` included (row K27). No pod is started, no Runpod API is
//! called and nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, Refusal, StartupRefusal, load, start_relaying};
use llm_runpod::{EmulatedRunpod, Identifier, ManualClock, PoolError, RunpodPool};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, VLLM_KEY, document, mutate, with_vllm_key};

const WAIT: Duration = Duration::from_secs(10);

/// A loopback pod: answers each connection with one fixed JSON body and records every byte it
/// read.
struct Pod {
    address: SocketAddr,
    received: Arc<Mutex<Vec<Vec<u8>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    fn start(answers: usize, body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&received);
        let thread = thread::spawn(move || {
            for _ in 0..answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let request = read_request(&mut connection);
                recording.lock().unwrap().push(request);
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self {
            address,
            received,
            thread: Some(thread),
        }
    }

    fn received(&mut self) -> Vec<Vec<u8>> {
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        self.received.lock().unwrap().clone()
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Every byte of one request: its head and its `content-length` body.
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

/// Reaches every pod at the loopback pod, and records the authority it was asked for.
struct Loopback {
    pod: SocketAddr,
    asked: Mutex<Vec<String>>,
}

impl PodConnector for Loopback {
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>> {
        self.asked.lock().unwrap().push(authority.to_owned());
        let stream = TcpStream::connect(self.pod)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

/// The support document listening on an ephemeral loopback port, serving every wire, with the
/// model's vLLM key file, and `edit` applied.
fn deployment(fixture: &Fixture, edit: impl Fn(String) -> String) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let text = mutate(
        &text,
        "wires = [\"chat\", \"responses\"]",
        "wires = [\"chat\", \"responses\", \"messages\"]",
    );
    load(&fixture.config(&edit(text))).unwrap()
}

/// One request to the gateway; the whole answer, read to the close.
fn post(address: SocketAddr, path: &str, bearer: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {bearer}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
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

// --- B8: the binary relays to the pool's pod with the model's vLLM key ------------------------

#[test]
fn b8_a_chat_request_reaches_the_pod_with_the_models_vllm_key_and_its_answer_reaches_the_client() {
    let fixture = Fixture::new("b8-relay");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"chat-1\",\"object\":\"chat.completion\"}");
    let loopback = Arc::new(Loopback {
        pod: pod.address,
        asked: Mutex::new(Vec::new()),
    });
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::clone(&loopback) as Arc<dyn PodConnector>,
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let answer = post(
        running.local_addr(),
        "/v1/chat/completions",
        OWNER_SECRET,
        "{\"model\":\"small\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
    );
    let report = running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    // An HTTP/1.1 client is answered chunked: one chunk holding the pod's body, then the end.
    assert!(
        answer.ends_with("\r\n{\"id\":\"chat-1\",\"object\":\"chat.completion\"}\r\n0\r\n\r\n"),
        "{answer}"
    );
    assert_eq!(report.accepted, report.completed);

    let received = pod.received();
    assert_eq!(received.len(), 1);
    let request = String::from_utf8(received[0].clone()).unwrap();
    assert!(
        request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"),
        "{request}"
    );
    assert_eq!(
        header(&request, "authorization"),
        Some(format!("Bearer {VLLM_KEY}").as_str()),
        "{request}"
    );
    assert_eq!(header(&request, "host"), Some("pod1-8000.proxy.runpod.net"));
    assert!(
        request.ends_with(
            "{\"model\":\"small\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"
        ),
        "{request}"
    );
    assert_eq!(
        *loopback.asked.lock().unwrap(),
        vec!["pod1-8000.proxy.runpod.net".to_owned()]
    );
    // One pod was created, and it reads the same key from the model's Runpod secret.
    let requests = runpod.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].env.get("VLLM_API_KEY").map(String::as_str),
        Some("{{ RUNPOD_SECRET_vllm_small }}")
    );
}

#[test]
fn b8_the_owner_credential_reaches_no_byte_the_pod_receives() {
    let fixture = Fixture::new("b8-owner");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(3, "{\"id\":\"any\"}");
    let running = start_relaying(
        &deployment,
        EmulatedRunpod::new(),
        Arc::new(Loopback {
            pod: pod.address,
            asked: Mutex::new(Vec::new()),
        }),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    for (path, body) in [
        (
            "/v1/chat/completions",
            "{\"model\":\"small\",\"messages\":[]}",
        ),
        ("/v1/responses", "{\"model\":\"small\",\"input\":\"hi\"}"),
        ("/v1/messages", "{\"model\":\"small\",\"messages\":[]}"),
    ] {
        let answer = post(running.local_addr(), path, OWNER_SECRET, body);
        assert!(answer.starts_with("HTTP/1.1 200 "), "{path}: {answer}");
    }
    running.shutdown();
    let received = pod.received();
    assert_eq!(received.len(), 3);
    for request in &received {
        assert!(
            find(request, OWNER_SECRET.as_bytes()).is_none(),
            "the owner's credential reached the pod: {}",
            String::from_utf8_lossy(request)
        );
        assert!(find(request, VLLM_KEY.as_bytes()).is_some());
    }
}

#[test]
fn b8_relaying_needs_every_models_vllm_key_file() {
    let fixture = Fixture::new("b8-no-key");
    let secret = fixture.owner_secret();
    let deployment = load(&fixture.config(&document("127.0.0.1:0", &secret))).unwrap();
    let refused: Refusal = start_relaying(
        &deployment,
        EmulatedRunpod::new(),
        Arc::new(Loopback {
            pod: "127.0.0.1:9".parse().unwrap(),
            asked: Mutex::new(Vec::new()),
        }),
        Arc::new(ManualClock::new(1_000)),
    )
    .err()
    .unwrap();
    assert_eq!(refused.kind(), StartupRefusal::ConfigValue);
    assert!(
        refused
            .message()
            .starts_with("models.small.vllm_api_key_file is required"),
        "{}",
        refused.message()
    );
}

#[test]
fn b8_the_shipped_binary_composes_no_emulator_and_no_relay_seam() {
    let main = include_str!("../src/main.rs");
    for forbidden in [
        "start_relaying",
        "Emulated",
        "PodConnector",
        "RunpodTransport",
    ] {
        assert!(!main.contains(forbidden), "src/main.rs names {forbidden}");
    }
    let manifest = include_str!("../Cargo.toml");
    assert!(
        !manifest.contains("[features]"),
        "a feature could select the emulator"
    );
}

// --- K27 and the pool's inputs ------------------------------------------------------------------

fn small() -> Identifier {
    Identifier::new("small").unwrap()
}

/// Asks until the pool hands out a ready lease, advancing the clock one second each time.
fn ready(pool: &RunpodPool<EmulatedRunpod>, clock: &ManualClock) -> llm_runpod::StreamLease {
    let authorization = llm_gateway_cli::compute_authorization().unwrap();
    for _ in 0..10 {
        match pool.ensure(&small(), &authorization) {
            Ok(lease) => return lease,
            Err(PoolError::Starting) => clock.advance(1_000),
            Err(other) => panic!("refused while starting: {}", other.code()),
        }
    }
    panic!("the pod never became ready");
}

#[test]
fn k27_an_idle_timeout_of_zero_stops_a_quiet_pod_at_the_first_pass_after_its_cold_start() {
    let fixture = Fixture::new("k27-zero");
    let deployment = deployment(&fixture, |text| {
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nidle_timeout_minutes = 0\n",
        )
    });
    assert_eq!(deployment.models["small"].idle_timeout_minutes, 0);
    let clock = ManualClock::new(1_000);
    let pool =
        llm_gateway_cli::runpod_pool(&deployment, EmulatedRunpod::new(), Arc::new(clock.clone()))
            .unwrap();
    // The cold start took one second, which is the floor of every idle limit.
    let lease = ready(&pool, &clock);
    assert!(
        pool.reap().unwrap().idle.is_empty(),
        "a request is in flight"
    );
    drop(lease);
    assert!(
        pool.reap().unwrap().idle.is_empty(),
        "stopped sooner than its measured cold start"
    );
    clock.advance(1_000);
    assert_eq!(pool.reap().unwrap().idle.len(), 1);
}

#[test]
fn k27_the_default_idle_timeout_keeps_a_quiet_pod_past_its_cold_start() {
    let fixture = Fixture::new("k27-default");
    let deployment = deployment(&fixture, |text| text);
    let clock = ManualClock::new(1_000);
    let pool =
        llm_gateway_cli::runpod_pool(&deployment, EmulatedRunpod::new(), Arc::new(clock.clone()))
            .unwrap();
    drop(ready(&pool, &clock));
    // Stepped once a minute, as the running gateway steps it, so its lease never runs out.
    for minute in 1..30 {
        clock.advance(60_000);
        assert!(
            pool.reap().unwrap().idle.is_empty(),
            "stopped after {minute} quiet minutes"
        );
    }
    clock.advance(60_000);
    assert_eq!(pool.reap().unwrap().idle.len(), 1);
}

#[test]
fn k27_each_pool_input_is_fixed_or_read_from_the_document() {
    let fixture = Fixture::new("k27-inputs");
    let deployment = deployment(&fixture, |text| {
        let text = mutate(
            &text,
            "kind = \"runpod-vllm\"\n",
            "kind = \"runpod-vllm\"\ncloud_type = \"COMMUNITY\"\n",
        );
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nidle_timeout_minutes = 7\nstart_wait_seconds = 90\ndisk_gb = 120\n",
        )
    });
    let policy = llm_gateway_cli::hosting_policy(&deployment).unwrap();
    assert_eq!(policy.controller.as_str(), "b10x-llm-gateway");
    assert_eq!(policy.provider.as_str(), "runpod");
    assert_eq!(policy.account.as_str(), "default");
    assert_eq!(policy.ledger.as_str(), "b10x-llm-gateway");
    assert_eq!(policy.max_active, 1, "one pod per declared model");
    assert_eq!(policy.max_lifetime_ms, 86_400_000);
    assert_eq!(policy.lease_ms, 300_000);
    assert!(
        policy.lease_ms > u64::try_from(llm_gateway_cli::CLEANUP_INTERVAL.as_millis()).unwrap(),
        "the pool must be stepped more often than its lease lasts"
    );
    let authorization = llm_gateway_cli::compute_authorization().unwrap();
    assert_eq!(authorization.ledger, policy.ledger);
    assert_eq!(authorization.reservation.as_str(), "unmetered");

    let models = llm_gateway_cli::runpod_models(&deployment).unwrap();
    let model = &models[&small()];
    assert_eq!(model.hf_model, "example/small-model");
    assert_eq!(model.image.as_str(), "vllm/vllm-openai:v0.27.1");
    assert_eq!(model.gpu_types, vec!["NVIDIA L40S".to_owned()]);
    assert_eq!(model.cloud_type, llm_runpod::CloudType::Community);
    assert_eq!(model.container_disk_gb, 120);
    assert_eq!(model.startup_deadline_ms, 90_000);
    assert_eq!(model.idle_timeout_ms, 7 * 60_000);
    assert_eq!(model.api_key_secret, "vllm_small");
    assert_eq!(model.crash_restart_limit, 2);
    assert_eq!(model.crash_window_ms, 600_000);
    assert_eq!(model.vllm, deployment.models["small"].vllm);
}
