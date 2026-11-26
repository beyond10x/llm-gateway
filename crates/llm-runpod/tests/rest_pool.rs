//! `RunpodPool` over `RestRunpod` against a loopback fake of Runpod's REST API v2: a `std`
//! listener on `127.0.0.1` that answers each request by its route from the state the pool's own
//! calls built (a create adds the pod, a delete removes it), and plays a recorded log stream for
//! `GET /v2/pods/{id}/logs`. Every request is checked against Runpod's published v2 document
//! (`tests/openapi.rs`). No call leaves the machine: the pod's `https` endpoint has no TLS
//! connection, so its readiness is never answered and it never becomes ready.

use std::{
    collections::BTreeMap,
    io::Write,
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    CloudType, ManualClock, PoolError, RestBinding, RestRunpod, RunpodModel, RunpodPool, Thinking,
    VllmSettings,
};
use serde_json::{Value, json};

use crate::{
    openapi::{check_answer, check_request},
    rest::{IMAGE, KEY, container_start, log_event, log_stream, page, pod, read_request},
};

const ALIAS: &str = "qwen";
/// How long the fake keeps a log stream open after its last event, as Runpod's stays open.
const STREAM_OPEN: Duration = Duration::from_secs(3);

fn id(value: &str) -> Identifier {
    Identifier::new(value).expect("test identifier")
}

/// What the fake holds: the account's pods, and the container starts its log stream reveals.
#[derive(Default)]
struct State {
    pods: Vec<Value>,
    /// Every container start of the one pod, oldest first. Log read `n` reveals the first `n`.
    starts: Vec<String>,
    log_reads: usize,
    terminated: Vec<String>,
    lines: Vec<String>,
}

struct Fake {
    address: String,
    state: Arc<Mutex<State>>,
}

impl Fake {
    /// A fake whose pod's container starts at each of `starts`, one more per log read.
    fn start(starts: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address").to_string();
        let state = Arc::new(Mutex::new(State {
            starts: starts.iter().map(|&start| start.to_owned()).collect(),
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let state = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &state));
            }
        });
        Self { address, state }
    }

    fn transport(&self) -> RestRunpod {
        RestRunpod::new(
            RestBinding {
                base_url: format!("http://{}/v2", self.address),
                graphql_url: format!("http://{}/graphql", self.address),
                ..RestBinding::runpod()
            },
            KEY.as_bytes().to_vec(),
        )
    }

    fn terminated(&self) -> Vec<String> {
        self.state.lock().expect("state").terminated.clone()
    }

    fn log_lines(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("state")
            .lines
            .iter()
            .filter(|line| line.contains("/logs"))
            .cloned()
            .collect()
    }
}

fn json_answer(status: u16, body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Answers one connection by its route.
fn serve(mut stream: TcpStream, state: &Mutex<State>) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    check_request(&request.line, &request.body);
    let target = request
        .line
        .split(' ')
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let method = request
        .line
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut state = state.lock().expect("state");
    state.lines.push(request.line.clone());
    let (bytes, open) = match (method.as_str(), path.as_str()) {
        ("POST", "/v2/pods") => {
            let body: Value = serde_json::from_slice(&request.body).expect("a create body");
            let tag = body["env"]["B10X_LLM_REQUEST"].as_str().unwrap_or_default();
            let mut created = pod("pod1", tag);
            created["name"] = body["name"].clone();
            created["image"] = body["image"].clone();
            created["env"] = body["env"].clone();
            created["gpu"]["id"] = body["gpu"]["id"].clone();
            check_answer("POST", "/v2/pods", 201, &created);
            state.pods.push(created.clone());
            (json_answer(201, &created), None)
        }
        ("GET", "/v2/pods") => {
            let listed = page(&state.pods, None);
            check_answer("GET", "/v2/pods", 200, &listed);
            (json_answer(200, &listed), None)
        }
        ("DELETE", "/v2/pods/pod1") => {
            state.pods.clear();
            state.terminated.push("pod1".to_owned());
            (
                b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec(),
                None,
            )
        }
        ("GET", "/v2/pods/pod1/logs") => {
            state.log_reads += 1;
            let shown = state.log_reads.min(state.starts.len());
            let mut events = Vec::new();
            for start in state.starts.iter().take(shown) {
                events.push(container_start(start));
                events.push(log_event(
                    "container",
                    "RuntimeError: Engine core initialization failed. See root cause above.",
                    start,
                ));
            }
            (log_stream(&events), Some(STREAM_OPEN))
        }
        _ => {
            let problem = json!({"title": "Not Found", "status": 404, "detail": target});
            (json_answer(404, &problem), None)
        }
    };
    drop(state);
    let _ = stream.write_all(&bytes);
    if let Some(open) = open {
        thread::sleep(open);
    }
}

fn policy() -> HostingPolicy {
    HostingPolicy {
        controller: id("controller-a"),
        provider: id("runpod"),
        account: id("account-1"),
        ledger: id("ledger-1"),
        max_active: 16,
        max_lifetime_ms: 86_400_000,
        lease_ms: 3_600_000,
    }
}

fn authorization() -> ComputeAuthorization {
    ComputeAuthorization {
        ledger: id("ledger-1"),
        reservation: id("reservation-1"),
    }
}

/// The gateway's fixed crash rule: two restarts inside ten minutes.
fn model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3.8-27B-FP8".to_owned(),
        image: id(IMAGE),
        gpu_types: vec!["NVIDIA L40S".to_owned()],
        cloud_type: CloudType::Secure,
        container_disk_gb: 80,
        cache: None,
        data_center_ids: Vec::new(),
        vllm: VllmSettings {
            max_model_len: 65_536,
            max_num_seqs: 8,
            gpu_memory_utilization: 0.9,
            thinking: Thinking::Off,
            reasoning_effort: None,
            sampling: None,
            extra_args: Vec::new(),
        },
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 1_800_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "default".to_owned(),
        fallbacks: Vec::new(),
    }
}

fn pool(fake: &Fake) -> (RunpodPool<RestRunpod>, ManualClock) {
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        fake.transport(),
        BTreeMap::from([(id(ALIAS), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    (pool, clock)
}

/// The owner's pod of 2026-11-26: its container started three times, 33 s apart, each start
/// followed by vLLM's engine failing, while the pod's `startedAt` never moved. Three starts
/// are two restarts, the limit, inside the ten-minute window: the pod is terminated and the
/// model answers `crash-loop`.
#[test]
fn a_container_restarting_past_the_limit_over_rest_is_terminated_and_answers_crash_loop() {
    let fake = Fake::start(&[
        "2026-11-26T15:29:38.112Z",
        "2026-11-26T15:30:11.204Z",
        "2026-11-26T15:30:44.318Z",
    ]);
    let (pool, clock) = pool(&fake);
    let mut answers = Vec::new();
    for _ in 0..8 {
        let answer = pool.ensure(&id(ALIAS), &authorization()).map(drop);
        answers.push(answer);
        if answer != Err(PoolError::Starting) {
            break;
        }
        clock.advance(10_000);
    }
    assert_eq!(
        answers,
        [
            Err(PoolError::Starting),
            Err(PoolError::Starting),
            Err(PoolError::Starting),
            Err(PoolError::CrashLoop),
        ],
        "created, first start seen, one restart, two restarts"
    );
    assert_eq!(fake.terminated(), ["pod1"]);
    assert_eq!(
        fake.log_lines(),
        ["GET /v2/pods/pod1/logs?source=system&tail=100 HTTP/1.1"; 3]
    );
}

/// The H100 pod of 2026-11-26: never ready, its container started once. Past its startup
/// deadline it is terminated by the next cleanup pass, with no request asking for it; at the
/// deadline itself it is not (the deadline is inclusive).
#[test]
fn a_pod_past_its_startup_deadline_that_never_became_ready_is_terminated_by_the_next_cleanup_pass()
{
    let fake = Fake::start(&["2026-11-26T15:29:38.112Z"]);
    let (pool, clock) = pool(&fake);
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    clock.advance(600_000);
    let at_deadline = pool.reap().expect("a cleanup pass");
    assert!(at_deadline.replaced.is_empty(), "{at_deadline:?}");
    assert_eq!(fake.terminated(), [] as [String; 0]);
    clock.advance(1);
    let past = pool.reap().expect("a cleanup pass");
    assert_eq!(past.replaced, [id("qwen-1")], "{past:?}");
    assert_eq!(fake.terminated(), ["pod1"]);
}
