//! Adversary cases for story:gateway-deployment (rows B8, K27, W7 and D2). Each case asserts
//! what the specification, `docs/hosting.md` or the capability matrix promise for the relay the
//! `start_relaying` seam composes. No pod is started, no Runpod API is called and nothing leaves
//! the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, load, runpod_pool, start_relaying};
use llm_runpod::{EmulatedRunpod, Identifier, ManualClock, PoolError};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, document, mutate, with_vllm_key};

/// The support document with the model's vLLM key file, and `edit` applied.
fn deployment(fixture: &Fixture, edit: impl Fn(String) -> String) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    load(&fixture.config(&edit(text))).unwrap()
}

/// One chat request to the gateway; the whole answer, read to the close.
fn chat(address: SocketAddr) -> String {
    let body = "{\"model\":\"small\",\"messages\":[]}";
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

/// A connector whose every connection is refused, as a pod the proxy cannot reach is. It records
/// the authority each connection was asked for.
#[derive(Default)]
struct Refusing {
    asked: Mutex<Vec<String>>,
}

impl PodConnector for Refusing {
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>> {
        self.asked.lock().unwrap().push(authority.to_owned());
        Err(io::Error::from(io::ErrorKind::ConnectionRefused))
    }
}

/// Matrix row D2 (covered): "The stop takes at most twice `read_timeout`". A request waiting
/// for its model's pod blocks in `acquire` for up to `start_wait_seconds` (10..=3600, default
/// 600), and the graceful stop joins it, so one request at a cold start holds the stop for the
/// whole wait.
#[test]
fn adv_d2_a_request_waiting_for_its_pod_cannot_hold_the_stop_past_twice_the_read_timeout() {
    let fixture = Fixture::new("adv-w03-d2-wait");
    let mut deployment = deployment(&fixture, |text| {
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nstart_wait_seconds = 10\n",
        )
    });
    deployment.gateway.read_timeout = Duration::from_secs(1);
    let runpod = EmulatedRunpod::new();
    // The pod never reports ready: a cold start longer than the stop's bound.
    runpod.ready_after(u32::MAX);
    let running = start_relaying(
        &deployment,
        runpod,
        Arc::new(Refusing::default()),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let waiting = thread::spawn(move || chat(address));
    thread::sleep(Duration::from_millis(1_500));
    let stopping = Instant::now();
    let report = running.shutdown();
    let took = stopping.elapsed();
    drop(waiting.join());
    assert_eq!(report.accepted, 1);
    assert!(
        took <= Duration::from_secs(2) + Duration::from_millis(500),
        "the stop took {took:?}, more than twice the 1 s read_timeout"
    );
}

/// `spec/domains/deployment.yaml` and `docs/hosting.md`: `idle_timeout_minutes = 0` stops the
/// pod "by the first cleanup pass that finds it with no request in flight". The request that
/// started the pod is still in flight at the gateway, polling `ensure` in `acquire`; when the
/// 60-second cleanup pass is the step that first observes the pod ready, the elapsed time since
/// the create equals the measured cold start, `>=` holds, and the pod is stopped before it
/// served anybody. The waiting request's next ask is then `Stopping`.
#[test]
fn adv_k27_an_idle_timeout_of_zero_never_stops_the_pod_a_waiting_request_started() {
    let fixture = Fixture::new("adv-w03-k27-waiting");
    let deployment = deployment(&fixture, |text| {
        mutate(
            &text,
            "max_model_len = 1024\n",
            "max_model_len = 1024\nidle_timeout_minutes = 0\n",
        )
    });
    let clock = ManualClock::new(1_000);
    let pool = runpod_pool(&deployment, EmulatedRunpod::new(), Arc::new(clock.clone())).unwrap();
    let small = Identifier::new("small").unwrap();
    let authorization = llm_gateway_cli::compute_authorization().unwrap();
    // The request arrives: its first ask creates the pod.
    assert!(matches!(
        pool.ensure(&small, &authorization),
        Err(PoolError::Starting)
    ));
    // The pod comes up during the request's wait, and the cleanup pass runs before its next ask.
    clock.advance(1_000);
    let pass = pool.reap().unwrap();
    assert!(
        pass.idle.is_empty(),
        "the cleanup pass stopped the pod the waiting request started, before it served it"
    );
    assert!(pool.ensure(&small, &authorization).is_ok());
}

/// Matrix row W7 (covered): "A transport failure or a proxy 502, 503 or 504 drops that endpoint
/// (only if it is still current) and answers 502; the next request starts a replacement". Row
/// W7 left the pool side to B8 ("No `RunpodPool` implements `RelayTargets` yet: that is B8"),
/// and the B8 composition's `invalidate` changes nothing, so every request after a failure is
/// sent to the same endpoint for as long as the pool's own probe still reports it ready.
#[test]
fn adv_w7_after_a_failed_connection_the_next_request_reaches_a_replacement() {
    let fixture = Fixture::new("adv-w03-w7");
    let deployment = deployment(&fixture, |text| text);
    let runpod = EmulatedRunpod::new();
    let connector = Arc::new(Refusing::default());
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::clone(&connector) as Arc<dyn PodConnector>,
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let first = chat(running.local_addr());
    let second = chat(running.local_addr());
    running.shutdown();
    assert!(first.starts_with("HTTP/1.1 502 "), "{first}");
    assert!(second.starts_with("HTTP/1.1 "), "{second}");
    let asked = connector.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_ne!(
        asked[0],
        asked[1],
        "the endpoint that failed was handed out again; {} pod(s) created",
        runpod.create_calls()
    );
}
