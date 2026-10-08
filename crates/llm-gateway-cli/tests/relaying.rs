//! The binary's relay to Runpod pods, composed in process through the `start_relaying` seam with
//! `EmulatedRunpod` and a loopback pod (row B8), and the pool's inputs from the deployment
//! document, `idle_timeout_minutes = 0` included (row K27), the hold of a request for a pod that
//! is still starting (rows L6, W6, K28), and the cleanup pass before serving and on its timer
//! (rows L15, L13), the pod and hold counters `GET /metrics` serves (rows O1, O2), and the refusal
//! of a tool request to a model without tool calling before a pod starts (`tools_*`). No pod is
//! started, no Runpod API is called and nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, Refusal, StartupRefusal, load, start_relaying};
use llm_runpod::{EmulatedRunpod, Identifier, ManualClock, PoolError, RunpodPool};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
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
    // Behaviour: the gateway `start` composes, which is what `main` runs, has no transport but
    // `ConnectorsRunpod` (story:live-runpod-wiring). This document names no connectors
    // connection, so the model is answered `target-unavailable` and no pool is asked. Were it
    // composed through `start_relaying` with the emulator, the emulated pod would answer.
    let fixture = Fixture::new("b8-shipped");
    let deployment = deployment(&fixture, |text| text);
    let running = llm_gateway_cli::start(&deployment).unwrap();
    let answer = post(
        running.local_addr(),
        "/v1/chat/completions",
        OWNER_SECRET,
        "{\"model\":\"small\",\"messages\":[]}",
    );
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(
        answer.contains("\"code\":\"target-unavailable\""),
        "{answer}"
    );
    // Source: only the seam's own module names the relay composition, and no source file
    // names the emulator, so neither `main` nor `start` can select it.
    let sources = [
        ("main.rs", include_str!("../src/main.rs")),
        ("lib.rs", include_str!("../src/lib.rs")),
        ("serve.rs", include_str!("../src/serve.rs")),
        ("config.rs", include_str!("../src/config.rs")),
        ("keys.rs", include_str!("../src/keys.rs")),
        ("refusal.rs", include_str!("../src/refusal.rs")),
        ("trusted.rs", include_str!("../src/trusted.rs")),
        ("relaying.rs", include_str!("../src/relaying.rs")),
    ];
    for (name, source) in sources {
        // Code only: a comment may name the emulator the tests use.
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!code.contains("Emulated"), "src/{name} uses the emulator");
        if name != "relaying.rs" && name != "lib.rs" {
            for forbidden in [
                "start_relaying",
                "bind_with_relay",
                "PoolTargets",
                "RunpodPool",
            ] {
                assert!(!code.contains(forbidden), "src/{name} names {forbidden}");
            }
        }
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

// --- L6, W6, K28: a request for a pod that is still starting -----------------------------------

const CHAT: &str = "/v1/chat/completions";
const SMALL_CHAT: &str = "{\"model\":\"small\",\"messages\":[]}";

fn loopback(pod: &Pod) -> Arc<dyn PodConnector> {
    Arc::new(Loopback {
        pod: pod.address,
        asked: Mutex::new(Vec::new()),
    })
}

/// The deployment with `lines` added to its `[models.small]` table.
fn with_model_lines(fixture: &Fixture, lines: &str) -> Deployment {
    deployment(fixture, |text| {
        mutate(
            &text,
            "max_model_len = 1024\n",
            &format!("max_model_len = 1024\n{lines}"),
        )
    })
}

/// Posts one chat request and measures how long the answer took.
fn timed_chat(address: SocketAddr) -> (String, Duration) {
    let started = Instant::now();
    let answer = post(address, CHAT, OWNER_SECRET, SMALL_CHAT);
    (answer, started.elapsed())
}

#[test]
fn l6_a_request_for_a_pod_still_starting_is_held_until_the_pod_serves() {
    let fixture = Fixture::new("l6-held");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"held\"}");
    let runpod = EmulatedRunpod::new();
    // Three probes answer "not ready": the pod takes at least three of the relay's asks to
    // serve, each 500 ms apart.
    runpod.ready_after(3);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, held) = timed_chat(running.local_addr());
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("{\"id\":\"held\"}"), "{answer}");
    assert!(
        held >= Duration::from_secs(1),
        "answered after {held:?}: the request was not held while the pod started"
    );
    assert_eq!(
        runpod.create_calls(),
        1,
        "a held request started a second pod"
    );
    assert_eq!(pod.received().len(), 1);
}

#[test]
fn l6_requests_held_together_wait_on_one_pod() {
    let fixture = Fixture::new("l6-together");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(3, "{\"id\":\"shared\"}");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(3);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let waiting: Vec<_> = (0..3)
        .map(|_| thread::spawn(move || timed_chat(address)))
        .collect();
    let answers: Vec<(String, Duration)> = waiting
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    running.shutdown();
    for (answer, _) in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    }
    assert_eq!(
        runpod.create_calls(),
        1,
        "held requests started more than one pod"
    );
    assert_eq!(pod.received().len(), 3);
}

#[test]
fn w6_a_pod_still_starting_past_the_hold_budget_is_model_cold_start_with_retry_after_30() {
    let fixture = Fixture::new("w6-cold");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 1\n");
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    // The pod never serves within this test.
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (first, held) = timed_chat(running.local_addr());
    let (second, _) = timed_chat(running.local_addr());
    running.shutdown();
    for answer in [&first, &second] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert!(answer.contains("\"code\":\"model-cold-start\""), "{answer}");
        assert_eq!(header(answer, "retry-after"), Some("30"), "{answer}");
    }
    assert!(
        held >= Duration::from_secs(1),
        "answered after {held:?}, before its 1 s hold budget"
    );
    assert!(
        held < Duration::from_secs(4),
        "answered after {held:?}, long past its 1 s hold budget"
    );
    // The pod keeps starting: the second request waited on it and started no other.
    assert_eq!(runpod.create_calls(), 1);
    assert!(runpod.terminations().is_empty());
    assert!(pod.received().is_empty());
}

#[test]
fn k28_the_request_hold_is_its_own_setting_and_defaults_to_start_wait_seconds() {
    let fixture = Fixture::new("k28-default");
    let unnamed = deployment(&fixture, |text| text);
    assert_eq!(unnamed.models["small"].start_wait_seconds, 600);
    assert_eq!(unnamed.models["small"].request_hold_seconds, 600);
    let longer = with_model_lines(&fixture, "start_wait_seconds = 90\n");
    assert_eq!(longer.models["small"].request_hold_seconds, 90);

    let named = with_model_lines(
        &fixture,
        "start_wait_seconds = 900\nrequest_hold_seconds = 45\n",
    );
    assert_eq!(named.models["small"].request_hold_seconds, 45);
    // The hold bounds a request, never the pod: the startup deadline is start_wait_seconds.
    let models = llm_gateway_cli::runpod_models(&named).unwrap();
    assert_eq!(models[&small()].startup_deadline_ms, 900_000);
}

#[test]
fn k28_the_request_hold_is_within_zero_and_the_startup_deadline() {
    let fixture = Fixture::new("k28-range");
    for admitted in ["0", "90"] {
        let lines = format!("start_wait_seconds = 90\nrequest_hold_seconds = {admitted}\n");
        let deployment = with_model_lines(&fixture, &lines);
        assert_eq!(
            deployment.models["small"].request_hold_seconds.to_string(),
            admitted
        );
    }
    let secret = fixture.owner_secret();
    let over = mutate(
        &document("127.0.0.1:0", &secret),
        "max_model_len = 1024\n",
        "max_model_len = 1024\nstart_wait_seconds = 90\nrequest_hold_seconds = 91\n",
    );
    let refused = load(&fixture.config(&over)).err().unwrap();
    assert_eq!(refused.kind(), StartupRefusal::ConfigValue);
    assert_eq!(
        refused.message(),
        "models.small.request_hold_seconds must be within 0..=90",
    );
}

#[test]
fn k28_a_hold_of_zero_answers_model_cold_start_at_once_and_still_starts_the_pod() {
    let fixture = Fixture::new("k28-zero");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 0\n");
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, held) = timed_chat(running.local_addr());
    running.shutdown();
    assert!(answer.contains("\"code\":\"model-cold-start\""), "{answer}");
    assert_eq!(header(&answer, "retry-after"), Some("30"));
    assert!(
        held < Duration::from_millis(500),
        "a hold of 0 waited {held:?}"
    );
    assert_eq!(
        runpod.create_calls(),
        1,
        "the request did not start the pod"
    );
    assert!(pod.received().is_empty());
}

// --- L15, L13: the cleanup pass ----------------------------------------------------------------

#[test]
fn l15_the_startup_sweep_terminates_a_previous_runs_pod_before_serving() {
    let fixture = Fixture::new("l15-sweep");
    let deployment = deployment(&fixture, |text| text);
    let runpod = EmulatedRunpod::new();
    // A previous run of this controller starts a pod and stops; the pod outlives it.
    let mut pod = Pod::start(1, "{\"id\":\"first-run\"}");
    let first = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, _) = timed_chat(first.local_addr());
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    first.shutdown();
    assert_eq!(pod.received().len(), 1);
    // Somebody else's pod is never swept.
    let foreign = runpod.insert_pod("someone-elses-pod", std::collections::BTreeMap::new());
    assert!(runpod.terminations().is_empty(), "stopping stops no pod");

    // The next run sweeps it before it serves anything: no request is made, and the timer's
    // first pass is a minute away.
    let second = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(2_000)),
    )
    .unwrap();
    let swept = runpod.terminations();
    second.shutdown();
    assert_eq!(swept, vec!["pod1".to_owned()]);
    assert!(!swept.contains(&foreign));
}

#[test]
fn l13_the_cleanup_pass_runs_every_60_seconds_well_inside_the_lease() {
    assert_eq!(llm_gateway_cli::CLEANUP_INTERVAL, Duration::from_secs(60));
    assert!(
        llm_gateway_cli::CLEANUP_INTERVAL.as_millis() * 5 <= u128::from(llm_gateway_cli::LEASE_MS),
        "five passes must fit in one lease, so a missed pass cannot lose it"
    );
}

// --- O1, O2: the binary's pod counters and the hold, read from `GET /metrics` ------------------

/// The samples of one `GET /metrics` the owner makes, `name{labels}` to value.
fn scrape(address: SocketAddr) -> std::collections::BTreeMap<String, String> {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "GET /metrics HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let (_, body) = answer.split_once("\r\n\r\n").unwrap();
    body.lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (series, value) = line.rsplit_once(' ').unwrap();
            (series.to_owned(), value.to_owned())
        })
        .collect()
}

#[test]
fn o1_o2_a_pod_start_and_the_request_held_on_it_are_counted() {
    let fixture = Fixture::new("o1-start");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"held\"}");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(3);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let before = scrape(running.local_addr());
    assert_eq!(before["llmgw_pod_starts_total"], "0");
    assert_eq!(
        before["llmgw_route_cold_start_holds_total{model=\"small\",wire=\"chat\"}"],
        "0"
    );
    let (answer, held) = timed_chat(running.local_addr());
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(pod.received().len(), 1);
    assert_eq!(after["llmgw_pod_starts_total"], "1");
    assert_eq!(after["llmgw_pod_start_failures_total"], "0");
    assert_eq!(after["llmgw_inference_requests_total"], "1");
    assert_eq!(
        after["llmgw_route_requests_total{model=\"small\",wire=\"chat\"}"],
        "1"
    );
    assert_eq!(
        after["llmgw_route_cold_start_holds_total{model=\"small\",wire=\"chat\"}"],
        "1"
    );
    let waited: f64 = after["llmgw_cold_start_wait_seconds_total"]
        .parse()
        .unwrap();
    assert!(
        waited >= 1.0 && waited <= held.as_secs_f64(),
        "waited {waited} s of a request held {held:?}"
    );
}

#[test]
fn o1_a_pod_start_refused_for_capacity_is_a_pod_start_failure() {
    let fixture = Fixture::new("o1-capacity");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    runpod.refuse_gpu("NVIDIA L40S");
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, _) = timed_chat(running.local_addr());
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(
        answer.contains("\"code\":\"target-unavailable\""),
        "{answer}"
    );
    assert_eq!(after["llmgw_pod_start_failures_total"], "1");
    assert_eq!(after["llmgw_pod_starts_total"], "0");
    assert_eq!(
        after["llmgw_route_refusals_total{model=\"small\",wire=\"chat\"}"],
        "1"
    );
    assert!(pod.received().is_empty());
}

#[test]
fn o1_the_startup_sweep_counts_each_pod_it_stops_as_a_reap() {
    let fixture = Fixture::new("o1-reap");
    let deployment = deployment(&fixture, |text| text);
    let runpod = EmulatedRunpod::new();
    let mut pod = Pod::start(1, "{\"id\":\"first-run\"}");
    let first = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, _) = timed_chat(first.local_addr());
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(scrape(first.local_addr())["llmgw_pod_reaps_total"], "0");
    first.shutdown();
    assert_eq!(pod.received().len(), 1);
    let second = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(2_000)),
    )
    .unwrap();
    let scraped = scrape(second.local_addr());
    second.shutdown();
    assert_eq!(runpod.terminations(), vec!["pod1".to_owned()]);
    assert_eq!(scraped["llmgw_pod_reaps_total"], "1");
    assert_eq!(scraped["llmgw_pod_starts_total"], "0");
}

// --- Tool calling: a tool request to a model without it starts no pod ---------------------------

#[test]
fn tools_a_tool_request_is_refused_for_the_absent_model_and_reaches_the_pod_for_the_parsed_one() {
    let fixture = Fixture::new("tools-relay");
    let key = fixture.vllm_key();
    let deployment = deployment(&fixture, |text| {
        let text = mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\ntool_calling = \"parsed\"\n",
        );
        format!(
            "{text}\n[models.text]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
             context_window = 65536\nhf_model = \"example/text-model\"\n\
             image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
             max_model_len = 1024\nvllm_api_key_file = \"{}\"\ntool_calling = \"absent\"\n",
            key.display()
        )
    });
    let mut pod = Pod::start(1, "{\"id\":\"tool\"}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let tools = "\"tools\":[{\"type\":\"function\",\"function\":{\"name\":\"shell\"}}]";
    let refused = post(
        running.local_addr(),
        CHAT,
        OWNER_SECRET,
        &format!("{{\"model\":\"text\",\"messages\":[],{tools}}}"),
    );
    assert_eq!(
        runpod.create_calls(),
        0,
        "a tool request to a model without tool calling started a pod"
    );
    let relayed = post(
        running.local_addr(),
        CHAT,
        OWNER_SECRET,
        &format!("{{\"model\":\"small\",\"messages\":[],{tools}}}"),
    );
    running.shutdown();

    assert!(refused.starts_with("HTTP/1.1 400 "), "{refused}");
    assert!(
        refused.contains(
            "{\"error\":{\"code\":\"tools-not-served\",\"message\":\"the model does not serve tool calls\"}}"
        ),
        "{refused}"
    );
    assert!(relayed.starts_with("HTTP/1.1 200 "), "{relayed}");
    assert!(relayed.contains("{\"id\":\"tool\"}"), "{relayed}");
    // Only the parsed model's pod was started, and it received the one tool request.
    assert_eq!(runpod.create_calls(), 1);
    let received = pod.received();
    assert_eq!(received.len(), 1);
    let request = String::from_utf8(received[0].clone()).unwrap();
    assert!(
        request.ends_with(&format!("{{\"model\":\"small\",\"messages\":[],{tools}}}")),
        "{request}"
    );
}

// --- R5, R1: the public routes read the document's models and start no pod --------------------

/// A `GET` without a credential, with `user-agent` when given; the whole answer.
fn public_get(address: SocketAddr, path: &str, user_agent: Option<&str>) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let agent = user_agent.map_or(String::new(), |agent| format!("user-agent: {agent}\r\n"));
    write!(stream, "GET {path} HTTP/1.1\r\nhost: gateway\r\n{agent}\r\n").unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

#[test]
fn r5_r1_the_binary_lists_and_profiles_each_model_from_the_document_and_starts_no_pod() {
    let fixture = Fixture::new("r5-r1-public");
    let deployment = deployment(&fixture, |text| {
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 2048\ntool_calling = \"parsed\"\n",
        )
    });
    let pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let listing = public_get(running.local_addr(), "/v1/models", None);
    let claude = public_get(running.local_addr(), "/", Some("claude-cli/2.0"));
    let codex = public_get(running.local_addr(), "/", Some("codex-cli/1.0"));
    let plain = public_get(running.local_addr(), "/", Some("curl/8.0"));
    running.shutdown();

    assert!(listing.starts_with("HTTP/1.1 200 "), "{listing}");
    assert!(
        listing.ends_with(
            "\r\n\r\n{\"object\":\"list\",\"data\":[{\"id\":\"small\",\"object\":\"model\",\
             \"owned_by\":\"llm-gateway\",\"max_model_len\":2048,\
             \"wires\":[\"chat\",\"responses\",\"messages\"]}]}"
        ),
        "{listing}"
    );
    assert!(claude.contains("export ANTHROPIC_MODEL='small'\n"), "{claude}");
    assert!(
        claude.contains("export CLAUDE_CODE_MAX_CONTEXT_TOKENS='65536'\n"),
        "{claude}"
    );
    assert!(codex.contains("model_context_window = 65536\n"), "{codex}");
    assert!(plain.contains("upstream_name = \"small\"\n"), "{plain}");
    assert!(plain.contains("context_window = 65536\n"), "{plain}");
    assert_eq!(runpod.create_calls(), 0, "a public route started a pod");
    for answer in [&listing, &claude, &codex, &plain] {
        for owner_only in [deployment.digest.as_str(), OWNER_SECRET, VLLM_KEY, "example/small-model"] {
            assert!(!answer.contains(owner_only), "{owner_only:?} in {answer}");
        }
    }
}
