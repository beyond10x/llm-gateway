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
use support::{
    Fixture, OWNER_SECRET, VLLM_KEY, document, hosted_table, mutate, no_spend, starts_on_request,
    with_start_on_request, with_vllm_key,
};

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

/// [`deployment`] whose call starts the pod ([`starts_on_request`]), and an emulator whose
/// account reports no spend: the composition every call started a pod in before
/// `start_on_request` existed.
fn starting(fixture: &Fixture, edit: impl Fn(String) -> String) -> (Deployment, EmulatedRunpod) {
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    (
        deployment(fixture, |text| starts_on_request(&edit(text))),
        runpod,
    )
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"chat-1\",\"object\":\"chat.completion\"}");
    let loopback = Arc::new(Loopback {
        pod: pod.address,
        asked: Mutex::new(Vec::new()),
    });
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(3, "{\"id\":\"any\"}");
    let running = start_relaying(
        &deployment,
        runpod,
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
    // A feature could select the emulator. The one feature the manifest declares, `test-seam`,
    // only builds the `gateway-tls-seam` test program: it enables no dependency, and no source
    // file of the library or `main` reads a feature.
    let manifest = include_str!("../Cargo.toml");
    let features = manifest
        .split_once("\n[features]\n")
        .map(|(_, rest)| rest.split_once("\n[").map_or(rest, |(table, _)| table));
    let declared: Vec<&str> = features
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(
        declared,
        ["test-seam = []"],
        "a feature could select the emulator"
    );
    let rest = [
        ("cli.rs", include_str!("../src/cli.rs")),
        ("logging.rs", include_str!("../src/logging.rs")),
        ("tls.rs", include_str!("../src/tls.rs")),
    ];
    for (name, source) in sources.into_iter().chain(rest) {
        assert!(
            !source.contains("cfg(feature"),
            "src/{name} reads a feature"
        );
    }
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

/// story:secure-cloud-enforced: a model whose image is not `vllm/vllm-openai:<tag>` is
/// `config:value`, and the refusal does not quote the image.
#[test]
fn an_image_outside_the_allowed_prefix_is_a_value_refusal() {
    let fixture = Fixture::new("image-not-allowed");
    for image in [
        "example/miner:v1",
        "vllm/vllm-openai:",
        "vllm/vllm-openai-fork:v1",
    ] {
        let deployment = deployment(&fixture, |text| {
            mutate(
                &text,
                "image = \"vllm/vllm-openai:v0.27.1\"",
                &format!("image = \"{image}\""),
            )
        });
        let refusal = llm_gateway_cli::runpod_models(&deployment).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{image}");
        assert!(!refusal.to_string().contains(image), "{image}: {refusal}");
    }
}

#[test]
fn k27_each_pool_input_is_fixed_or_read_from_the_document() {
    let fixture = Fixture::new("k27-inputs");
    let deployment = deployment(&fixture, |text| {
        let text = mutate(
            &text,
            "kind = \"runpod-vllm\"\n",
            "kind = \"runpod-vllm\"\ncloud_type = \"SECURE\"\n",
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
    assert_eq!(model.cloud_type, llm_runpod::CloudType::Secure);
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
    deployment(fixture, |text| model_lines(&text, lines))
}

/// `text` with `lines` added to its `[models.small]` table.
fn model_lines(text: &str, lines: &str) -> String {
    mutate(
        text,
        "max_model_len = 1024\n",
        &format!("max_model_len = 1024\n{lines}"),
    )
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"held\"}");
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(3, "{\"id\":\"shared\"}");
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
    let (deployment, runpod) = starting(&fixture, |text| {
        model_lines(&text, "request_hold_seconds = 1\n")
    });
    let mut pod = Pod::start(0, "{}");
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
    let (deployment, runpod) = starting(&fixture, |text| {
        model_lines(&text, "request_hold_seconds = 0\n")
    });
    let mut pod = Pod::start(0, "{}");
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
    let (deployment, runpod) = starting(&fixture, |text| text);
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"held\"}");
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
    let (deployment, runpod) = starting(&fixture, |text| text);
    let mut pod = Pod::start(0, "{}");
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
    let (deployment, runpod) = starting(&fixture, |text| text);
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
    let (deployment, runpod) = starting(&fixture, |text| {
        let text = mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\ntool_calling = \"parsed\"\n",
        );
        format!(
            "{text}\n[models.text]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
             context_window = 65536\nhf_model = \"example/text-model\"\n\
             image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
             max_model_len = 1024\nvllm_api_key_file = \"{}\"\ntool_calling = \"absent\"\n\
             price_per_hour_usd = 2\nstart_on_request = true\n",
            key.display()
        )
    });
    let mut pod = Pod::start(1, "{\"id\":\"tool\"}");
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
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: gateway\r\n{agent}\r\n"
    )
    .unwrap();
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
    // The binary always translates (story:gateway-translation), so every wire is listed, and
    // the document's own wires follow as `upstream_wires`.
    assert!(
        listing.ends_with(
            "\r\n\r\n{\"object\":\"list\",\"data\":[{\"id\":\"small\",\"object\":\"model\",\
             \"owned_by\":\"llm-gateway\",\"max_model_len\":2048,\
             \"wires\":[\"chat\",\"responses\",\"messages\"],\
             \"upstream_wires\":[\"chat\",\"responses\",\"messages\"]}]}"
        ),
        "{listing}"
    );
    assert!(
        claude.contains("export ANTHROPIC_MODEL='small'\n"),
        "{claude}"
    );
    assert!(
        claude.contains("export CLAUDE_CODE_MAX_CONTEXT_TOKENS='65536'\n"),
        "{claude}"
    );
    assert!(codex.contains("model_context_window = 65536\n"), "{codex}");
    assert!(plain.contains("upstream_name = \"small\"\n"), "{plain}");
    assert!(plain.contains("context_window = 65536\n"), "{plain}");
    assert_eq!(runpod.create_calls(), 0, "a public route started a pod");
    for answer in [&listing, &claude, &codex, &plain] {
        for owner_only in [
            deployment.digest.as_str(),
            OWNER_SECRET,
            VLLM_KEY,
            "example/small-model",
        ] {
            assert!(!answer.contains(owner_only), "{owner_only:?} in {answer}");
        }
    }
}

// --- A declared effort reaches a hosted endpoint, never a Runpod pod (story:effort-default) ---

/// A messages request naming no effort reaches the hosted model `open`, which declares
/// `reasoning_effort = "low"`, with `output_config.effort` `low`, and reaches the Runpod model
/// `small`, which declares `reasoning_effort = "medium"` as its server's chat-template default,
/// with only its `model` as sent: the binary gives a declared effort to a hosted model only. A
/// request naming an effort keeps it, and the chat wire gains none.
#[test]
fn effort_a_hosted_model_gives_its_declared_effort_and_a_runpod_model_never_does() {
    let fixture = Fixture::new("effort-default");
    let mut endpoint = Pod::start(3, "{\"id\":\"hosted-1\"}");
    let base_url = format!("http://{}", endpoint.address);
    let (deployment, runpod) = starting(&fixture, |text| {
        let text = mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nreasoning_effort = \"medium\"\n",
        );
        format!(
            "{text}\n{}",
            hosted_table(
                "open",
                &base_url,
                "auth_kind = \"anonymous\"\nreasoning_effort = \"low\"\n"
            )
        )
    });
    let mut pod = Pod::start(1, "{\"id\":\"pod-1\"}");
    let loopback = Arc::new(Loopback {
        pod: pod.address,
        asked: Mutex::new(Vec::new()),
    });
    let running = start_relaying(
        &deployment,
        runpod,
        Arc::clone(&loopback) as Arc<dyn PodConnector>,
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let answers = [
        post(
            address,
            "/v1/messages",
            OWNER_SECRET,
            "{\"model\":\"small\",\"messages\":[]}",
        ),
        post(
            address,
            "/v1/messages",
            OWNER_SECRET,
            "{\"model\":\"open\",\"messages\":[]}",
        ),
        post(
            address,
            "/v1/messages",
            OWNER_SECRET,
            "{\"model\":\"open\",\"output_config\":{\"effort\":\"medium\"},\"messages\":[]}",
        ),
        post(
            address,
            "/v1/chat/completions",
            OWNER_SECRET,
            "{\"model\":\"open\",\"messages\":[]}",
        ),
    ];
    running.shutdown();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    }
    let body = |raw: &Vec<u8>| {
        let text = String::from_utf8(raw.clone()).unwrap();
        text.split_once("\r\n\r\n").unwrap().1.to_owned()
    };
    let runpod: Vec<String> = pod.received().iter().map(body).collect();
    assert_eq!(runpod, ["{\"model\":\"small\",\"messages\":[]}"]);
    let hosted: Vec<String> = endpoint.received().iter().map(body).collect();
    assert_eq!(
        hosted,
        [
            "{\"model\":\"open\",\"messages\":[],\"output_config\":{\"effort\":\"low\"}}",
            "{\"model\":\"open\",\"output_config\":{\"effort\":\"medium\"},\"messages\":[]}",
            "{\"model\":\"open\",\"messages\":[]}",
        ]
    );
}

// --- The output clamp's window (story:max-tokens-clamp) ---------------------------------------

/// Every request a pod has read so far, each as `<request line> <body>`, without waiting for the
/// rest of its script.
fn requests_so_far(pod: &Pod) -> Vec<String> {
    pod.received
        .lock()
        .unwrap()
        .iter()
        .map(|raw| {
            let text = String::from_utf8_lossy(raw).into_owned();
            let line = text.split("\r\n").next().unwrap_or_default().to_owned();
            let body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
            format!("{line} {body}")
        })
        .collect()
}

/// Criterion 1: the binary's window for a Runpod model is its `max_model_len` (1024), not its
/// `context_window` (65536); a hosted model's is its `context_window` (4096); a hosted model
/// declaring none has no window, so its request is relayed with no count. Every pod counts the
/// input at 1000 tokens.
#[test]
fn clamp_a_runpod_model_is_clamped_to_its_max_model_len_a_hosted_one_to_its_context_window() {
    const COUNT: &str = "{\"input_tokens\":1000}";
    let fixture = Fixture::new("max-tokens-clamp");
    let open = Pod::start(2, COUNT);
    let bare = Pod::start(2, COUNT);
    let (open_url, bare_url) = (
        format!("http://{}", open.address),
        format!("http://{}", bare.address),
    );
    let (deployment, runpod) = starting(&fixture, |text| {
        format!(
            "{text}\n{}\n{}",
            hosted_table(
                "open",
                &open_url,
                "auth_kind = \"anonymous\"\ncontext_window = 4096\n"
            ),
            hosted_table("bare", &bare_url, "auth_kind = \"anonymous\"\n"),
        )
    });
    let pod = Pod::start(2, COUNT);
    let running = start_relaying(
        &deployment,
        runpod,
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let answers: Vec<String> = ["small", "open", "bare"]
        .iter()
        .map(|alias| {
            post(
                address,
                "/v1/messages",
                OWNER_SECRET,
                &format!("{{\"model\":\"{alias}\",\"max_tokens\":32000,\"messages\":[]}}"),
            )
        })
        .collect();
    running.shutdown();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    }
    let count = |alias: &str| {
        format!(
            "POST /v1/messages/count_tokens HTTP/1.1 {{\"model\":\"{alias}\",\"max_tokens\":32000,\"messages\":[]}}"
        )
    };
    let relay = |alias: &str, max_tokens: u32| {
        format!(
            "POST /v1/messages HTTP/1.1 {{\"model\":\"{alias}\",\"max_tokens\":{max_tokens},\"messages\":[]}}"
        )
    };
    assert_eq!(requests_so_far(&pod), [count("small"), relay("small", 24)]);
    assert_eq!(requests_so_far(&open), [count("open"), relay("open", 3096)]);
    assert_eq!(requests_so_far(&bare), [relay("bare", 32000)]);
}

// --- story:spend-caps: a pod start that would pass a spend ceiling ---------------------------

/// The deployment with a daily ceiling of 10 USD on its provider and a price of 2 USD an hour
/// on `small`, whose default 600-second startup deadline makes a start estimate 0.333334 USD.
fn capped(fixture: &Fixture) -> Deployment {
    deployment(fixture, |text| {
        let text = mutate(
            &text,
            "kind = \"runpod-vllm\"\n",
            "kind = \"runpod-vllm\"\ndaily_cap_usd = 10\n",
        );
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nprice_per_hour_usd = 2\n",
        )
    })
}

/// [`capped`] whose call starts the pod ([`with_start_on_request`]).
fn capped_on_request(fixture: &Fixture) -> Deployment {
    deployment(fixture, |text| {
        let text = mutate(
            &text,
            "kind = \"runpod-vllm\"\n",
            "kind = \"runpod-vllm\"\ndaily_cap_usd = 10\n",
        );
        with_start_on_request(&mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nprice_per_hour_usd = 2\n",
        ))
    })
}

fn spent(runpod: &EmulatedRunpod, daily_micro_usd: u64) {
    runpod.set_account_spend(Some(llm_runpod::AccountSpend {
        daily_micro_usd,
        total_micro_usd: daily_micro_usd,
    }));
}

#[test]
fn spend_a_start_past_the_ceiling_is_spend_ceiling_at_once_with_no_create_and_is_counted() {
    let fixture = Fixture::new("spend-over");
    let deployment = capped_on_request(&fixture);
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    spent(&runpod, 9_666_667);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, took) = timed_chat(running.local_addr());
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("\"code\":\"spend-ceiling\""), "{answer}");
    assert_eq!(header(&answer, "retry-after"), None, "{answer}");
    assert!(took < Duration::from_secs(5), "not held: {took:?}");
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(runpod.spend_reads(), 1);
    assert_eq!(after["llmgw_pod_start_spend_refusals_total"], "1");
    assert_eq!(after["llmgw_pod_start_failures_total"], "0");
    assert_eq!(after["llmgw_pod_starts_total"], "0");
    assert_eq!(
        after["llmgw_route_refusals_total{model=\"small\",wire=\"chat\"}"],
        "1"
    );
    assert!(pod.received().is_empty());
}

#[test]
fn spend_a_start_at_the_ceiling_is_created_and_served() {
    let fixture = Fixture::new("spend-at");
    let deployment = capped_on_request(&fixture);
    let mut pod = Pod::start(1, "{\"id\":\"served\"}");
    let runpod = EmulatedRunpod::new();
    spent(&runpod, 9_666_666);
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
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(after["llmgw_pod_start_spend_refusals_total"], "0");
    assert_eq!(pod.received().len(), 1);
}

#[test]
fn spend_the_pool_is_metered_exactly_when_a_ceiling_is_declared() {
    let fixture = Fixture::new("spend-ledger");
    let plain = deployment(&fixture, |text| text);
    let unmetered = llm_gateway_cli::compute_authorization_for(&plain).unwrap();
    assert_eq!(unmetered.reservation.as_str(), "unmetered");
    assert!(!llm_gateway_cli::spend_policy(&plain).unwrap().is_metered());
    let deployment = capped(&fixture);
    let metered = llm_gateway_cli::compute_authorization_for(&deployment).unwrap();
    assert_eq!(metered.ledger.as_str(), "b10x-llm-gateway");
    assert_eq!(metered.reservation.as_str(), "spend-ceiling");
    let policy = llm_gateway_cli::spend_policy(&deployment).unwrap();
    assert!(policy.is_metered());
    assert_eq!(policy.daily_cap_micro_usd, Some(10_000_000));
    assert_eq!(policy.total_cap_micro_usd, None);
    assert_eq!(policy.price_per_hour_micro_usd[&small()], 2_000_000);
}

// ---- story:model-profile-fallback: the usage record names the profile that served ----

/// Everything the test binary's global subscriber writes, kept in memory.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The binary's records as its default filter logs them, from every test of this binary: a
/// test reads only the lines of its own model alias.
fn usage_log() -> &'static Captured {
    static LOG: std::sync::OnceLock<Captured> = std::sync::OnceLock::new();
    LOG.get_or_init(|| {
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(llm_gateway_cli::log_filter(None))
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();
        captured
    })
}

#[test]
fn a_relayed_call_names_the_profile_of_the_pod_that_served() {
    let log = usage_log();
    let fixture = Fixture::new("profile-record");
    // The call starts the pod: `starts_on_request` edits `[models.small]` before it is renamed.
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let deployment = deployment(&fixture, |text| {
        let text = starts_on_request(&text);
        let text = mutate(&text, "[models.small]", "[models.profiled]");
        let text = mutate(
            &text,
            "gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n",
            "",
        );
        format!(
            "{text}\n[[models.profiled.profiles]]\nname = \"l40s-1k\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n\n[[models.profiled.profiles]]\nname = \"h100-2k\"\ngpu_types = [\"NVIDIA H100 NVL\"]\nmax_model_len = 2048\n"
        )
    });
    let pod = Pod::start(1, "{\"id\":\"chat-1\",\"object\":\"chat.completion\"}");
    runpod.no_capacity_gpu("NVIDIA L40S");
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let answer = post(
        running.local_addr(),
        "/v1/chat/completions",
        OWNER_SECRET,
        "{\"model\":\"profiled\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}",
    );
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let gpus: Vec<String> = runpod
        .requests()
        .iter()
        .map(|request| request.gpu_type.clone())
        .collect();
    assert_eq!(gpus, ["NVIDIA L40S", "NVIDIA H100 NVL"]);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let records: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("usage record") && line.contains("model=\"profiled\""))
        .collect();
    assert_eq!(records.len(), 1, "{text}");
    assert!(records[0].contains("profile=\"h100-2k\""), "{}", records[0]);
}

// --- story:a-call-starts-no-pod: `start_on_request` --------------------------------------------

/// The fixed message of `model-stopped`, as the client reads it.
const MODEL_STOPPED: &str = "{\"error\":{\"code\":\"model-stopped\",\"message\":\"the model's pod is not running and a request does not start it; the owner wakes it\"}}";

/// The owner's wake of `small`: `POST /v1/models/small/wake` with no body.
fn wake(address: SocketAddr) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST /v1/models/small/wake HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// The emulator, except that every terminate is refused, so a stopped pod stays `stopping`.
#[derive(Clone)]
struct TerminateRefused(EmulatedRunpod);

impl llm_runpod::RunpodTransport for TerminateRefused {
    fn list_pods(&mut self) -> Result<llm_runpod::PodListing, ()> {
        self.0.list_pods()
    }
    fn create_pod(&mut self, request: &llm_runpod::PodRequest) -> llm_runpod::CreateAnswer {
        self.0.create_pod(request)
    }
    fn terminate_pod(&mut self, _pod_id: &str) -> llm_runpod::TerminateAnswer {
        llm_runpod::TerminateAnswer::Refused
    }
    fn probe_ready(&mut self, target: &llm_runpod::ProbeTarget<'_>) -> llm_runpod::Probe {
        self.0.probe_ready(target)
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.0.container_started_at(pod_id)
    }
    fn pod_placement(&mut self, pod_id: &str) -> Option<llm_runpod::PodPlacement> {
        self.0.pod_placement(pod_id)
    }
    fn account_spend(&mut self, day_start_ms: u64) -> Option<llm_runpod::AccountSpend> {
        self.0.account_spend(day_start_ms)
    }
}

/// The document of [`deployment`] with `start_on_request = true`, a daily cap of 10 USD on its
/// provider and a price of 2 USD an hour on `small`, and `lines` added to `[models.small]`.
fn starting_on_request(fixture: &Fixture, lines: &str) -> Deployment {
    deployment(fixture, |text| {
        let text = mutate(
            &text,
            "kind = \"runpod-vllm\"\n",
            "kind = \"runpod-vllm\"\ndaily_cap_usd = 10\n",
        );
        mutate(
            &text,
            "max_model_len = 1024\n",
            &format!(
                "max_model_len = 1024\nprice_per_hour_usd = 2\nstart_on_request = true\n{lines}"
            ),
        )
    })
}

/// Acceptance 2: without the key, a call to a model whose pod is stopped is `model-stopped` at
/// once on every wire, without `retry-after`; no pod is created, no spend is read, no byte
/// reaches a pod (the messages call's output clamp counts nothing), and each call is counted
/// `refused` and held on no route.
#[test]
fn start_on_request_off_a_call_to_a_stopped_model_is_model_stopped_at_once_and_creates_nothing() {
    let fixture = Fixture::new("sor-stopped");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let started = Instant::now();
    let answers: Vec<(&str, String)> = [
        (CHAT, SMALL_CHAT),
        ("/v1/responses", "{\"model\":\"small\",\"input\":\"hi\"}"),
        (
            "/v1/messages",
            "{\"model\":\"small\",\"max_tokens\":5,\"messages\":[]}",
        ),
    ]
    .into_iter()
    .map(|(path, body)| (path, post(address, path, OWNER_SECRET, body)))
    .collect();
    let took = started.elapsed();
    let after = scrape(address);
    running.shutdown();
    for (path, answer) in &answers {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{path}: {answer}");
        assert!(answer.ends_with(MODEL_STOPPED), "{path}: {answer}");
        assert_eq!(header(answer, "retry-after"), None, "{path}: {answer}");
    }
    assert!(
        took < Duration::from_secs(2),
        "the calls were held {took:?}"
    );
    assert_eq!(runpod.create_calls(), 0, "a call started a pod");
    assert_eq!(runpod.spend_reads(), 0);
    assert!(pod.received().is_empty());
    assert_eq!(after["llmgw_pod_starts_total"], "0");
    assert_eq!(
        after["llmgw_requests_total{model=\"small\",outcome=\"refused\"}"],
        "3"
    );
    assert_eq!(
        after["llmgw_route_cold_start_holds_total{model=\"small\",wire=\"chat\"}"],
        "0"
    );
}

/// Acceptance 3 and 4: without the key the owner's wake starts the pod, and a call while it
/// starts is held on that pod and relayed once it serves, with no second create.
#[test]
fn start_on_request_off_a_call_after_the_owners_wake_is_held_on_the_starting_pod_and_relayed() {
    let fixture = Fixture::new("sor-woken");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"woken\"}");
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
    let woken = wake(address);
    let (answer, held) = timed_chat(address);
    let after = scrape(address);
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(woken.contains("\"phase\":\"starting\""), "{woken}");
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("{\"id\":\"woken\"}"), "{answer}");
    assert!(
        held >= Duration::from_millis(500),
        "answered after {held:?}: the call was not held while the pod started"
    );
    assert_eq!(runpod.create_calls(), 1, "the call started a second pod");
    assert_eq!(pod.received().len(), 1);
    assert_eq!(after["llmgw_pod_starts_total"], "1");
    assert_eq!(
        after["llmgw_route_cold_start_holds_total{model=\"small\",wire=\"chat\"}"],
        "1"
    );
}

/// Acceptance 3: without the key, a call held on a pod the wake started that is still starting
/// when its hold passes is `model-cold-start` with `retry-after: 30`, as today.
#[test]
fn start_on_request_off_a_call_held_past_its_hold_after_the_wake_is_model_cold_start() {
    let fixture = Fixture::new("sor-cold");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 1\n");
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
    let address = running.local_addr();
    let woken = wake(address);
    let (answer, held) = timed_chat(address);
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
    assert!(answer.contains("\"code\":\"model-cold-start\""), "{answer}");
    assert_eq!(header(&answer, "retry-after"), Some("30"), "{answer}");
    assert!(
        held >= Duration::from_secs(1) && held < Duration::from_secs(4),
        "answered after {held:?}, not after its 1 s hold"
    );
    assert_eq!(runpod.create_calls(), 1);
    assert!(pod.received().is_empty());
}

/// Acceptance 3: without the key, a pod that fails while a call is held on it (a crash loop)
/// loses the hold: that call is `target-unavailable`, as a lost hold is today, and starts no
/// replacement; the next call is `model-stopped` at once, and still nothing is created.
#[test]
fn start_on_request_off_a_pod_that_fails_while_a_call_is_held_starts_no_replacement() {
    let fixture = Fixture::new("sor-failed");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 8\n");
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(u32::MAX);
    runpod.uptimes(vec![30, 5, 40, 2, 50, 1]);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let woken = wake(address);
    let (held_call, _) = timed_chat(address);
    let (next, took) = timed_chat(address);
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(held_call.starts_with("HTTP/1.1 503 "), "{held_call}");
    assert!(
        held_call.contains("\"code\":\"target-unavailable\""),
        "{held_call}"
    );
    assert!(next.ends_with(MODEL_STOPPED), "{next}");
    assert!(
        took < Duration::from_secs(2),
        "the next call was held {took:?}"
    );
    assert_eq!(runpod.create_calls(), 1, "a replacement was started");
    assert_eq!(runpod.terminations(), ["pod1"]);
    assert!(pod.received().is_empty());
}

/// Acceptance 2: without the key, a call to a model whose pod owes a stop that is not confirmed
/// (`stopping`) is `model-stopped` at once and starts nothing.
#[test]
fn start_on_request_off_a_call_to_a_stopping_model_is_model_stopped_at_once() {
    let fixture = Fixture::new("sor-stopping");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 2\n");
    // The pod accepts no connection: the first call through it fails and stops it.
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        TerminateRefused(runpod.clone()),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let woken = wake(address);
    let (failed, _) = timed_chat(address);
    let (next, took) = timed_chat(address);
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(failed.contains("\"code\":\"upstream-failed\""), "{failed}");
    assert!(next.starts_with("HTTP/1.1 503 "), "{next}");
    assert!(next.ends_with(MODEL_STOPPED), "{next}");
    assert_eq!(header(&next, "retry-after"), None, "{next}");
    assert!(took < Duration::from_secs(1), "the call was held {took:?}");
    assert_eq!(runpod.create_calls(), 1);
    assert!(pod.received().is_empty());
}

/// Acceptance 5: with the key and a daily cap, a call to a stopped model starts its pod and is
/// held until it serves, call for call as before the key existed.
#[test]
fn start_on_request_on_with_a_cap_a_call_starts_the_pod_and_is_held_as_before() {
    let fixture = Fixture::new("sor-on");
    let deployment = starting_on_request(&fixture, "");
    let mut pod = Pod::start(1, "{\"id\":\"started\"}");
    let runpod = EmulatedRunpod::new();
    spent(&runpod, 0);
    runpod.ready_after(3);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let (answer, held) = timed_chat(running.local_addr());
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("{\"id\":\"started\"}"), "{answer}");
    assert!(
        held >= Duration::from_secs(1),
        "answered after {held:?}: the call was not held while its pod started"
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(runpod.spend_reads(), 1);
    assert_eq!(pod.received().len(), 1);
    assert_eq!(after["llmgw_pod_starts_total"], "1");
}

// --- adversary, story:a-call-starts-no-pod: who may start a pod --------------------------------

/// Opens no connection to any pod, probe or relay: the shipped-composition cases below reach
/// nothing past loopback.
struct AdvNoPod;

impl PodConnector for AdvNoPod {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no pod in this test",
        ))
    }
}

/// `text` (a [`document`]) with a daily cap of 10 USD on `[providers.runpod]`, a price of 2 USD
/// an hour and a hold of 2 s on `[models.small]`, and no `start_on_request`.
fn adv_capped_without_the_key(text: &str) -> String {
    let text = mutate(
        text,
        "kind = \"runpod-vllm\"\n",
        "kind = \"runpod-vllm\"\ndaily_cap_usd = 10\n",
    );
    model_lines(&text, "price_per_hour_usd = 2\nrequest_hold_seconds = 2\n")
}

/// Acceptance 1 (default `false`) and 2: a daily cap on the provider, which the owner declares
/// for the wake, is not `start_on_request`. A call to the stopped model is `model-stopped` at
/// once, creates nothing and reads no spend; the owner's wake then starts the pod under the
/// spend check, so the composition could start one.
#[test]
fn adv_w02_a_daily_cap_without_the_key_lets_no_call_start_a_pod_and_the_wake_still_does() {
    let fixture = Fixture::new("adv-w02-cap-no-key");
    let deployment = deployment(&fixture, |text| adv_capped_without_the_key(&text));
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let (refused, took) = timed_chat(address);
    let creates_after_call = runpod.create_calls();
    let reads_after_call = runpod.spend_reads();
    let woken = wake(address);
    running.shutdown();
    assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
    assert!(refused.ends_with(MODEL_STOPPED), "{refused}");
    assert_eq!(header(&refused, "retry-after"), None, "{refused}");
    assert!(took < Duration::from_secs(1), "the call was held {took:?}");
    assert_eq!(
        creates_after_call, 0,
        "a call started a pod: the provider's daily cap acted as start_on_request"
    );
    assert_eq!(reads_after_call, 0, "a call read the account's spend");
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert_eq!(runpod.create_calls(), 1, "the owner's wake started no pod");
    assert_eq!(runpod.spend_reads(), 1);
    assert!(pod.received().is_empty());
}

/// Acceptance 2 and 5 on one provider: `big` sets `start_on_request`, `small` does not. A call
/// to `big` starts `big`'s pod and is relayed; a call to `small`, before and after, is
/// `model-stopped` and starts nothing. Exactly one pod is created, and it is `big`'s.
#[test]
fn adv_w02_a_second_model_that_sets_the_key_starts_only_its_own_pod() {
    let fixture = Fixture::new("adv-w02-two-models");
    let key = fixture.vllm_key();
    let deployment = deployment(&fixture, |text| {
        let text = adv_capped_without_the_key(&text);
        format!(
            "{text}\n[models.big]\nprovider = \"runpod\"\nwires = [\"chat\"]\n\
             context_window = 65536\nhf_model = \"example/big-model\"\n\
             image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
             max_model_len = 1024\nvllm_api_key_file = \"{}\"\nprice_per_hour_usd = 2\n\
             request_hold_seconds = 8\nstart_on_request = true\n",
            key.display()
        )
    });
    let mut pod = Pod::start(1, "{\"id\":\"big\"}");
    let runpod = EmulatedRunpod::new();
    no_spend(&runpod);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let (before, _) = timed_chat(address);
    let big = post(
        address,
        CHAT,
        OWNER_SECRET,
        "{\"model\":\"big\",\"messages\":[]}",
    );
    let (after, took) = timed_chat(address);
    running.shutdown();
    for answer in [&before, &after] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert!(answer.ends_with(MODEL_STOPPED), "{answer}");
    }
    assert!(took < Duration::from_secs(1), "the call was held {took:?}");
    assert!(big.starts_with("HTTP/1.1 200 "), "{big}");
    assert!(big.contains("{\"id\":\"big\"}"), "{big}");
    let names: Vec<String> = runpod
        .requests()
        .into_iter()
        .map(|request| request.name)
        .collect();
    assert_eq!(names.len(), 1, "creates: {names:?}");
    assert!(
        names[0].contains("big") && !names[0].contains("small"),
        "creates: {names:?}"
    );
    assert_eq!(pod.received().len(), 1);
}

/// Acceptance 3, the retry after a failed relay: the owner wakes the pod, the first call through
/// it fails (`upstream-failed`, which stops the pod), and the next call is `model-stopped` and
/// starts no replacement, with the stop accepted.
#[test]
fn adv_w02_a_call_after_the_woken_pod_failed_a_relay_starts_no_replacement() {
    let fixture = Fixture::new("adv-w02-invalidated");
    let deployment = with_model_lines(&fixture, "request_hold_seconds = 4\n");
    // The pod accepts no connection: the first call through it fails and stops it.
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        loopback(&pod),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let woken = wake(address);
    let (failed, _) = timed_chat(address);
    let (next, took) = timed_chat(address);
    let (third, _) = timed_chat(address);
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(failed.contains("\"code\":\"upstream-failed\""), "{failed}");
    for answer in [&next, &third] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert!(answer.ends_with(MODEL_STOPPED), "{answer}");
    }
    assert!(took < Duration::from_secs(1), "the call was held {took:?}");
    assert_eq!(runpod.create_calls(), 1, "a replacement was started");
    assert_eq!(runpod.terminations().len(), 1);
    assert!(pod.received().is_empty());
}

/// The `connectors` script a pod start through the shipped composition needs: a listing, a
/// placement read and one applied create of `small`'s pod.
fn adv_connectors_script() -> serde_json::Value {
    serde_json::json!({
        "invoke pods.list": [support::listed(&serde_json::json!([support::running_small_pod()]))],
        "invoke pods.list machine": [support::listed(&serde_json::json!([support::running_small_pod()]))],
        "invoke pod.create": [support::created(&support::running_small_pod())],
    })
}

/// Acceptance 2 in the composition the shipped binary runs (`start_connected` over
/// `ConnectorsRunpod` and the `connectors` fixture): without the key a chat call sends no
/// `pod.create` and is `model-stopped`; the owner's wake then sends exactly one.
#[test]
fn adv_w02_the_shipped_composition_without_the_key_sends_no_create_on_a_call_and_one_on_a_wake() {
    let fixture = Fixture::new("adv-w02-shipped");
    let connectors = support::Connectors::new(&fixture);
    connectors.script(&adv_connectors_script());
    let text = support::connected_document(&fixture, "127.0.0.1:0", &connectors, 2);
    let deployment = load(&fixture.config(&text)).unwrap();
    let relaying =
        llm_gateway_cli::start_connected(&deployment, |transport| transport, Arc::new(AdvNoPod))
            .unwrap();
    let address = relaying.local_addr();
    let (refused, took) = timed_chat(address);
    let creates_after_call = connectors.calls_of("invoke pod.create").len();
    let approvals_after_call = connectors.calls_of("approvals prepare pod.create").len();
    let woken = wake(address);
    relaying.shutdown();
    assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
    assert!(refused.ends_with(MODEL_STOPPED), "{refused}");
    assert!(took < Duration::from_secs(1), "the call was held {took:?}");
    assert_eq!(creates_after_call, 0, "a call sent pod.create");
    assert_eq!(approvals_after_call, 0, "a call prepared a pod.create");
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert_eq!(
        connectors.calls_of("invoke pod.create").len(),
        1,
        "the owner's wake sent no pod.create: {:?}",
        connectors.calls()
    );
}

/// `adversary_w12p2_another_providers_ceiling_does_not_bind_the_connected_account`
/// (`adversary_w12_spend_pass2.rs`) asserts that its call reads no spend and is not
/// `spend-ceiling`. Since this story its call starts no pod at all (`model-stopped`), so it
/// passes whatever the composition does with the other provider's ceiling. The same contract
/// (`docs/hosting.md`, "Spend ceilings": "A ceiling declared on another provider does not bind
/// the pool ... when the connected provider declares none, the pool reads no spend") driven
/// through the start that still exists without the key, the owner's wake: it sends one create
/// and reads no spend.
#[test]
fn adv_w02_another_providers_ceiling_does_not_bind_the_owners_wake_on_the_connected_account() {
    let fixture = Fixture::new("adv-w02-foreign-ceiling-wake");
    let connectors = support::Connectors::new(&fixture);
    connectors.script(&adv_connectors_script());
    let text = support::connected_document(&fixture, "127.0.0.1:0", &connectors, 1);
    let text = mutate(
        &text,
        "\n[models.small]\n",
        "\n[providers.reserve]\nkind = \"runpod-vllm\"\ndaily_cap_usd = 1\n\n[models.small]\n",
    );
    let text = model_lines(&text, "price_per_hour_usd = 2\n");
    let deployment = load(&fixture.config(&text)).unwrap();
    let relaying =
        llm_gateway_cli::start_connected(&deployment, |transport| transport, Arc::new(AdvNoPod))
            .unwrap();
    let address = relaying.local_addr();
    let (call, _) = timed_chat(address);
    let woken = wake(address);
    relaying.shutdown();
    // The old case's call no longer reaches a start: this is why it cannot fail any more.
    assert!(call.ends_with(MODEL_STOPPED), "{call}");
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(!woken.contains("spend-ceiling"), "{woken}");
    assert_eq!(
        connectors.calls_of("invoke billing.pods").len(),
        0,
        "another provider's ceiling was checked against the connected account"
    );
    assert_eq!(connectors.calls_of("invoke pod.create").len(), 1);
}
