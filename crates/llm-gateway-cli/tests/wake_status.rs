//! The owner's lifecycle routes in the binary's composition (story:model-wake-and-status):
//! `POST /v1/models/{alias}/wake` starts a Runpod model's pod through the same pool path a
//! held request takes, so the spend check and the profile fallback apply, and
//! `GET /v1/models/{alias}/status` reads the phase from the pool without a provider call.
//! Composed in process through the `start_relaying` seam with `EmulatedRunpod` and a loopback
//! pod; no Runpod API is called and nothing leaves the loopback interface.
//!
//! story:ollama-route: an Ollama model's status, its acquisition, its calls and its usage
//! records, against `FakeOllama`, a loopback server at port 0 answering as Ollama does. No test
//! reaches a real Ollama, and none pulls or loads a model.

mod support;

use llm_gateway::{
    Gateway, GatewayConfig, Label, ModelKind, ModelPhase, ModelStatus, OwnerToken, Relay,
    RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier,
    TargetRefusal, Wire,
};
use llm_gateway_cli::{
    Deployment, EndpointCredential, HostedEndpoint, HostedTargets, PodConnector, ProbeBounds,
    Resolver, load, start_relaying,
};
use llm_runpod::{AccountSpend, EmulatedRunpod, ManualClock};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use support::{Fixture, OWNER_SECRET, document, hosted_table, mutate, with_vllm_key};

const WAIT: Duration = Duration::from_secs(10);
const SMALL_CHAT: &str =
    "{\"model\":\"small\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}";

/// A loopback pod answering `answers` connections with one fixed JSON body.
struct Pod {
    address: SocketAddr,
    thread: Option<JoinHandle<usize>>,
}

impl Pod {
    fn start(answers: usize, body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = thread::spawn(move || {
            let mut served = 0;
            for _ in 0..answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return served;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let mut seen = Vec::new();
                let mut chunk = [0_u8; 4096];
                while !seen.ends_with(b"}") {
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => seen.extend_from_slice(&chunk[..read]),
                    }
                }
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                drop(connection.write_all(answer.as_bytes()));
                served += 1;
            }
            served
        });
        Self {
            address,
            thread: Some(thread),
        }
    }

    fn served(&mut self) -> usize {
        self.thread
            .take()
            .map_or(0, |thread| thread.join().unwrap())
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

/// The support document on an ephemeral port, with the model's vLLM key file and `edit`.
fn deployment(fixture: &Fixture, edit: impl Fn(String) -> String) -> Deployment {
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    load(&fixture.config(&edit(text))).unwrap()
}

/// One request with the owner credential; the whole answer, read to the close.
fn send(address: SocketAddr, method: &str, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let length = if body.is_empty() {
        String::new()
    } else {
        format!(
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\n{length}\r\n{body}"
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn scrape(address: SocketAddr) -> BTreeMap<String, String> {
    let answer = send(address, "GET", "/metrics", "");
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    body(&answer)
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.rsplit_once(' '))
        .map(|(series, value)| (series.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn wake_starts_the_pod_once_at_once_status_reads_it_without_a_call_and_a_request_reuses_it() {
    let fixture = Fixture::new("wake-start");
    let deployment = deployment(&fixture, |text| text);
    let mut pod = Pod::start(1, "{\"id\":\"woken\"}");
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(2);
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let stopped = send(address, "GET", "/v1/models/small/status", "");
    let started = Instant::now();
    let woken = send(address, "POST", "/v1/models/small/wake", "");
    let took = started.elapsed();
    let again = send(address, "POST", "/v1/models/small/wake", "");
    let lists = runpod.lists();
    let starting = send(address, "GET", "/v1/models/small/status", "");
    let lists_after_status = runpod.lists();
    let relayed = send(address, "POST", "/v1/chat/completions", SMALL_CHAT);
    let ready = send(address, "GET", "/v1/models/small/status", "");
    let after = scrape(address);
    running.shutdown();
    assert!(stopped.starts_with("HTTP/1.1 200 "), "{stopped}");
    assert_eq!(
        body(&stopped),
        "{\"model\":\"small\",\"phase\":\"stopped\"}"
    );
    assert!(woken.starts_with("HTTP/1.1 202 Accepted\r\n"), "{woken}");
    assert_eq!(
        body(&woken),
        "{\"model\":\"small\",\"phase\":\"starting\",\"sub_phase\":\"running\",\"profile\":\"default\"}"
    );
    assert!(took < Duration::from_secs(2), "the wake waited {took:?}");
    assert!(again.starts_with("HTTP/1.1 202 "), "{again}");
    assert!(body(&again).contains("\"phase\":\"starting\""), "{again}");
    assert_eq!(body(&starting), body(&again), "status observed anew");
    assert_eq!(lists_after_status, lists, "a status listed the pods");
    assert!(relayed.starts_with("HTTP/1.1 200 "), "{relayed}");
    assert!(relayed.contains("{\"id\":\"woken\"}"), "{relayed}");
    assert_eq!(
        body(&ready),
        "{\"model\":\"small\",\"phase\":\"ready\",\"profile\":\"default\"}"
    );
    assert_eq!(
        runpod.create_calls(),
        1,
        "a wake or the request created again"
    );
    assert_eq!(pod.served(), 1);
    assert_eq!(after["llmgw_pod_starts_total"], "1");
    assert_eq!(
        after["llmgw_inference_requests_total"], "1",
        "a lifecycle route made a usage record"
    );
}

#[test]
fn a_wake_past_the_spend_ceiling_is_spend_ceiling_at_once_with_no_create_and_is_counted() {
    let fixture = Fixture::new("wake-spend");
    let deployment = deployment(&fixture, |text| {
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
    });
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    runpod.set_account_spend(Some(AccountSpend {
        daily_micro_usd: 9_666_667,
        total_micro_usd: 9_666_667,
    }));
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let refused = send(running.local_addr(), "POST", "/v1/models/small/wake", "");
    let status = send(running.local_addr(), "GET", "/v1/models/small/status", "");
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(refused.starts_with("HTTP/1.1 503 "), "{refused}");
    assert!(refused.contains("\"code\":\"spend-ceiling\""), "{refused}");
    assert!(
        !refused.to_ascii_lowercase().contains("retry-after"),
        "{refused}"
    );
    assert_eq!(body(&status), "{\"model\":\"small\",\"phase\":\"stopped\"}");
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(runpod.spend_reads(), 1);
    assert_eq!(after["llmgw_pod_start_spend_refusals_total"], "1");
    assert_eq!(after["llmgw_pod_starts_total"], "0");
    assert_eq!(pod.served(), 0);
}

#[test]
fn a_wake_without_capacity_starts_the_next_profile_and_status_names_it() {
    let fixture = Fixture::new("wake-profile");
    let deployment = deployment(&fixture, |text| {
        let text = mutate(
            &text,
            "gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n",
            "",
        );
        format!(
            "{text}\n[[models.small.profiles]]\nname = \"l40s-1k\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n\n[[models.small.profiles]]\nname = \"h100-2k\"\ngpu_types = [\"NVIDIA H100 NVL\"]\nmax_model_len = 2048\n"
        )
    });
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    runpod.no_capacity_gpu("NVIDIA L40S");
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let woken = send(running.local_addr(), "POST", "/v1/models/small/wake", "");
    let status = send(running.local_addr(), "GET", "/v1/models/small/status", "");
    running.shutdown();
    assert!(woken.starts_with("HTTP/1.1 202 "), "{woken}");
    assert!(body(&woken).contains("\"profile\":\"h100-2k\""), "{woken}");
    assert!(
        body(&status).contains("\"profile\":\"h100-2k\""),
        "{status}"
    );
    let gpus: Vec<String> = runpod
        .requests()
        .iter()
        .map(|request| request.gpu_type.clone())
        .collect();
    assert_eq!(gpus, ["NVIDIA L40S", "NVIDIA H100 NVL"]);
    assert_eq!(pod.served(), 0);
}

/// story:route-status-for-every-model: a hosted model has no wake, and its status is its
/// endpoint's probe; nothing listens at port 9, so it is `unreachable` `no-answer`.
#[test]
fn a_hosted_model_has_no_wake_and_its_status_is_its_probe_beside_a_pod_model() {
    let fixture = Fixture::new("wake-hosted");
    let deployment = deployment(&fixture, |text| {
        format!(
            "{text}\n{}",
            hosted_table(
                "routed",
                "http://127.0.0.1:9/v1",
                "upstream_model = \"vendor-routed\"\nauth_kind = \"anonymous\"\n"
            )
        )
    });
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let wake = send(address, "POST", "/v1/models/routed/wake", "");
    let status = send(address, "GET", "/v1/models/routed/status", "");
    let pod_status = send(address, "GET", "/v1/models/small/status", "");
    running.shutdown();
    assert!(wake.starts_with("HTTP/1.1 404 "), "{wake}");
    assert!(wake.contains("\"code\":\"path-unknown\""), "{wake}");
    assert!(status.starts_with("HTTP/1.1 200 "), "{status}");
    assert_eq!(
        body(&status),
        "{\"model\":\"routed\",\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\"}"
    );
    assert!(pod_status.starts_with("HTTP/1.1 200 "), "{pod_status}");
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(pod.served(), 0);
}

/// A loopback endpoint that answers every connection with `status` and counts them.
struct Answering {
    address: SocketAddr,
    probes: Arc<AtomicUsize>,
}

impl Answering {
    fn start(status: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let probes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&probes);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut seen = [0_u8; 4096];
                drop(connection.read(&mut seen));
                drop(write!(
                    connection,
                    "HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                ));
            }
        });
        Self { address, probes }
    }

    fn probes(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
    }
}

/// Acceptance 5 in the binary's composition: `GET /v1/status` lists the pod model, the hosted
/// model and the route of the routes document in alias order with their kinds, probes each
/// endpoint once for two reads, and starts nothing.
#[test]
fn route_status_the_binary_lists_a_pod_a_hosted_model_and_a_route_with_their_kinds() {
    let fixture = Fixture::new("route-status-list");
    let hosted = Answering::start("200 OK");
    let route = Answering::start("401 Unauthorized");
    let routes = fixture.write(
        "routes.yaml",
        format!(
            "format: llm.routes/1\nendpoints:\n  open:\n    base_url: http://{}/v1\n    protocol: chat-completions\n    auth: {{kind: anonymous}}\nroutes:\n  router: open/vendor/model-x\n",
            route.address
        )
        .as_bytes(),
        0o600,
    );
    let deployment = deployment(&fixture, |text| {
        let text = mutate(
            &text,
            "\n[providers.runpod]",
            &format!("routes = \"{}\"\n\n[providers.runpod]", routes.display()),
        );
        format!(
            "{text}\n{}",
            hosted_table(
                "remote",
                &format!("http://{}/v1", hosted.address),
                "upstream_model = \"vendor-remote\"\nauth_kind = \"anonymous\"\n"
            )
        )
    });
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let list = send(running.local_addr(), "GET", "/v1/status", "");
    let again = send(running.local_addr(), "GET", "/v1/status", "");
    let after = scrape(running.local_addr());
    running.shutdown();
    assert!(list.starts_with("HTTP/1.1 200 "), "{list}");
    assert_eq!(
        body(&list),
        concat!(
            "{\"format\":\"llm-gateway.status/1\",\"models\":[",
            "{\"model\":\"remote\",\"kind\":\"hosted\",\"wires\":[\"chat\",\"messages\"],\"phase\":\"ready\",\"callable\":true},",
            "{\"model\":\"router\",\"kind\":\"route\",\"wires\":[\"chat\"],\"phase\":\"unreachable\",\"sub_phase\":\"credential-refused\",\"callable\":false},",
            "{\"model\":\"small\",\"kind\":\"pod\",\"wires\":[\"chat\",\"responses\"],\"phase\":\"stopped\",\"callable\":true}",
            "]}"
        )
    );
    assert_eq!(body(&again), body(&list));
    assert_eq!(
        (hosted.probes(), route.probes()),
        (1, 1),
        "a result was not reused"
    );
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(pod.served(), 0);
    assert_eq!(after["llmgw_inference_requests_total"], "0");
}

/// Accepts every connection, holds it open and never answers.
fn silent_endpoint() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let mut held = Vec::new();
        for connection in listener.incoming() {
            match connection {
                Ok(connection) => held.push(connection),
                Err(_) => return,
            }
        }
    });
    address
}

/// The rest of the composition: no model of its own.
struct Nothing;

impl RelayTargets for Nothing {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// Acceptance 5: three hosted endpoints that accept and never answer are listed within one
/// probe deadline plus one second, each `unreachable` `no-answer` and not callable. The deadline
/// is shortened to one second here; the published one is held by
/// `route_status_the_published_probe_bounds_are_the_defaults` in `src/hosted.rs`.
#[test]
fn route_status_three_endpoints_that_never_answer_are_listed_within_one_deadline_and_a_second() {
    let deadline = Duration::from_secs(1);
    let mut endpoints = BTreeMap::new();
    let mut models = Vec::new();
    for alias in ["one", "three", "two"] {
        let address = silent_endpoint();
        endpoints.insert(
            alias.to_owned(),
            HostedEndpoint::new(
                &format!("http://{address}/v1"),
                EndpointCredential::Anonymous,
            )
            .unwrap(),
        );
        models.push(
            RelayModel::new(
                Label::new(alias).unwrap(),
                Label::new(alias).unwrap(),
                vec![Wire::Chat],
            )
            .unwrap()
            .with_kind(ModelKind::Hosted),
        );
    }
    let targets = HostedTargets::new(
        endpoints,
        Arc::new(Loopback(silent_endpoint())),
        Arc::new(Nothing),
    )
    .with_probe_bounds(ProbeBounds {
        timeout: deadline,
        ..ProbeBounds::default()
    });
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        Relay::new(models, Arc::new(targets)).unwrap(),
    )
    .unwrap();
    handle.mark_ready();
    let started = Instant::now();
    let list = send(handle.local_addr(), "GET", "/v1/status", "");
    let took = started.elapsed();
    handle.shutdown();
    assert!(list.starts_with("HTTP/1.1 200 "), "{list}");
    let entry = |alias: &str| {
        format!(
            "{{\"model\":\"{alias}\",\"kind\":\"hosted\",\"wires\":[\"chat\"],\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\",\"callable\":false}}"
        )
    };
    assert_eq!(
        body(&list),
        format!(
            "{{\"format\":\"llm-gateway.status/1\",\"models\":[{},{},{}]}}",
            entry("one"),
            entry("three"),
            entry("two")
        )
    );
    assert!(took >= deadline, "{took:?}");
    assert!(
        took < deadline + Duration::from_secs(1),
        "three silent endpoints took {took:?}"
    );
}

/// A resolver whose lookup of port 9 never returns, counting those calls; any other address
/// literal is answered as it is written.
fn stuck_resolver(calls: &Arc<AtomicUsize>) -> Resolver {
    let calls = Arc::clone(calls);
    Arc::new(move |host: &str, port: u16| {
        if port != 9 {
            return Ok(vec![SocketAddr::new(host.parse().unwrap(), port)]);
        }
        calls.fetch_add(1, Ordering::SeqCst);
        loop {
            thread::park();
        }
    })
}

/// The name lookup is inside the probe's deadline: a model whose lookup never returns is
/// `unreachable` `no-answer` at the deadline, and the list with it beside a silent endpoint keeps
/// its bound of one deadline and a second. A second list read starts no second lookup.
#[test]
fn route_status_a_lookup_that_never_returns_is_no_answer_and_the_list_keeps_its_bound() {
    let deadline = Duration::from_secs(1);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut endpoints = BTreeMap::new();
    let mut models = Vec::new();
    for (alias, address) in [
        ("lost", "127.0.0.1:9".to_owned()),
        ("silent", silent_endpoint().to_string()),
    ] {
        endpoints.insert(
            alias.to_owned(),
            HostedEndpoint::new(
                &format!("http://{address}/v1"),
                EndpointCredential::Anonymous,
            )
            .unwrap(),
        );
        models.push(
            RelayModel::new(
                Label::new(alias).unwrap(),
                Label::new(alias).unwrap(),
                vec![Wire::Chat],
            )
            .unwrap()
            .with_kind(ModelKind::Hosted),
        );
    }
    let targets = HostedTargets::new(
        endpoints,
        Arc::new(Loopback(silent_endpoint())),
        Arc::new(Nothing),
    )
    .with_probe_bounds(ProbeBounds {
        timeout: deadline,
        reuse: Duration::ZERO,
    })
    .with_resolver(stuck_resolver(&calls));
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        Relay::new(models, Arc::new(targets)).unwrap(),
    )
    .unwrap();
    handle.mark_ready();
    let mut took = Vec::new();
    let mut lists = Vec::new();
    for _ in 0..2 {
        let started = Instant::now();
        lists.push(send(handle.local_addr(), "GET", "/v1/status", ""));
        took.push(started.elapsed());
    }
    handle.shutdown();
    let entry = |alias: &str| {
        format!(
            "{{\"model\":\"{alias}\",\"kind\":\"hosted\",\"wires\":[\"chat\"],\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\",\"callable\":false}}"
        )
    };
    for (list, took) in lists.iter().zip(&took) {
        assert!(list.starts_with("HTTP/1.1 200 "), "{list}");
        assert_eq!(
            body(list),
            format!(
                "{{\"format\":\"llm-gateway.status/1\",\"models\":[{},{}]}}",
                entry("lost"),
                entry("silent")
            )
        );
        assert!(
            *took < deadline + Duration::from_secs(1),
            "a lookup that never returns held the list {took:?}"
        );
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a second lookup started while the first still ran"
    );
}

// --- The launch route asks no endpoint (story:route-status-for-every-model, correction 1) -----

const LISTENING: &str = "b10x-llm-gateway: listening on ";
const STARTUP: Duration = Duration::from_secs(20);

/// A loopback endpoint that accepts every connection, counts it, holds it open and never
/// answers.
struct Silent {
    address: SocketAddr,
    accepted: Arc<AtomicUsize>,
}

impl Silent {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&accepted);
        thread::spawn(move || {
            let mut held = Vec::new();
            for connection in listener.incoming() {
                let Ok(connection) = connection else { return };
                counted.fetch_add(1, Ordering::SeqCst);
                held.push(connection);
            }
        });
        Self { address, accepted }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

fn launch_directory(name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("launch-{name}"));
    drop(std::fs::remove_dir_all(&dir));
    std::fs::create_dir_all(dir.join("out")).unwrap();
    std::fs::create_dir_all(dir.join("runtime")).unwrap();
    for path in [dir.clone(), dir.join("runtime")] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    dir
}

/// An open loopback deployment listening on `listen`, serving the hosted model `qwen` (the
/// launcher's default implementer route) from `endpoint`.
fn launch_config(
    dir: &std::path::Path,
    name: &str,
    listen: &str,
    endpoint: SocketAddr,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "listen = \"{listen}\"\ningress_auth = \"none\"\n\n[hosted.qwen]\nwires = [\"chat\", \"messages\"]\n\
             base_url = \"http://{endpoint}/v1\"\nupstream_model = \"vendor-qwen\"\n\
             auth_kind = \"anonymous\"\ntool_calling = \"parsed\"\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    path
}

/// The shipped binary serving `config`; killed when dropped.
struct Served {
    child: std::process::Child,
    address: SocketAddr,
    _lines: std::sync::mpsc::Receiver<String>,
}

impl Served {
    fn start(config: &std::path::Path) -> Self {
        use std::io::BufRead as _;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
            .args(["--config", config.to_str().unwrap()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, lines) = std::sync::mpsc::channel();
        thread::spawn(move || {
            for line in io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + STARTUP;
        let mut written = Vec::new();
        let address = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(line) = lines.recv_timeout(left) else {
                drop(child.kill());
                drop(child.wait());
                panic!("the gateway never reported {LISTENING:?}; stderr: {written:?}");
            };
            written.push(line.clone());
            if let Some(address) = line.strip_prefix(LISTENING) {
                break address.parse().unwrap();
            }
        };
        Self {
            child,
            address,
            _lines: lines,
        }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// `GET /v1/launch` asks no endpoint: with one hosted model whose endpoint accepts and never
/// answers, the launch route is answered 200 and the endpoint has accepted no connection when the
/// answer has arrived.
#[test]
fn route_status_the_launch_route_asks_no_endpoint() {
    let dir = launch_directory("route-asks-no-endpoint");
    let endpoint = Silent::start();
    let server = Served::start(&launch_config(
        &dir,
        "gateway.toml",
        "127.0.0.1:0",
        endpoint.address,
    ));
    let mut stream = TcpStream::connect(server.address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "GET /v1/launch HTTP/1.0\r\nhost: {}\r\naccept: application/json\r\nconnection: close\r\n\r\n",
        server.address
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    let accepted = endpoint.accepted();
    drop(server);
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert!(answer.contains("\"alias\":\"qwen\""), "{answer}");
    assert_eq!(
        accepted, 0,
        "the launch route probed the endpoint: {answer}"
    );
}

/// The launcher starts Claude Code (`tests/fixture/claude.rs`) against a gateway whose
/// implementer route's endpoint never answers, exits 0, and no connection reached the endpoint.
#[test]
fn route_status_the_launcher_starts_claude_without_a_probe_of_its_routes() {
    let dir = launch_directory("launcher-no-probe");
    let endpoint = Silent::start();
    let server = Served::start(&launch_config(
        &dir,
        "gateway.toml",
        "127.0.0.1:0",
        endpoint.address,
    ));
    let launcher_config = launch_config(
        &dir,
        "launcher.toml",
        &server.address.to_string(),
        endpoint.address,
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .arg("claude")
        .arg("--config")
        .arg(&launcher_config)
        .env(
            "B10X_LLM_GATEWAY_CLAUDE_BIN",
            env!("CARGO_BIN_EXE_claude-fixture"),
        )
        .env("FAKE_CLAUDE_OUT", dir.join("out"))
        .env("FAKE_CLAUDE_EXIT", "0")
        .env("XDG_RUNTIME_DIR", dir.join("runtime"))
        .env_remove("ANTHROPIC_BASE_URL")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
        .env_remove("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        .env_remove("CLAUDE_CODE_MAX_CONTEXT_TOKENS")
        .output()
        .unwrap();
    let accepted = endpoint.accepted();
    drop(server);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(
        accepted, 0,
        "the launch probed the endpoint; stderr: {stderr}"
    );
}

// --- An opening left running past the probe deadline (story:route-status-for-every-model,
// correction 2) -------------------------------------------------------------------------------

mod opening_left_running {
    //! Adversarial pass 2 over story:route-status-for-every-model: the status probe's opening on its
    //! own thread (`HostedTargets`, correction 1). An `https` opening goes through the connector, so
    //! a connector of this file stands in for a TLS handshake that ends after the probe's deadline.
    //! Every endpoint is a loopback listener on port 0; nothing leaves the loopback interface.

    use llm_gateway::{
        ModelPhase, ModelStatus, RelayStream, RelayTarget, RelayTargets, TargetRefusal,
    };
    use llm_gateway_cli::{
        EndpointCredential, HostedEndpoint, HostedTargets, PodConnector, ProbeBounds,
    };
    use std::{
        collections::BTreeMap,
        io::{self, Read, Write},
        net::{SocketAddr, TcpListener, TcpStream},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    const DEADLINE: Duration = Duration::from_millis(300);

    /// What a loopback endpoint saw of one connection.
    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Accepted,
        /// The gateway closed the connection without sending a request.
        ClosedIdle,
        /// A request head arrived and was answered.
        Answered,
    }

    /// A loopback endpoint answering each request head with `answer`, reporting what it saw of
    /// each connection on the returned channel.
    fn endpoint(answer: &'static str) -> (SocketAddr, mpsc::Receiver<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, seen) = mpsc::channel();
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let sender = sender.clone();
                drop(sender.send(Seen::Accepted));
                thread::spawn(move || {
                    connection
                        .set_read_timeout(Some(Duration::from_secs(20)))
                        .unwrap();
                    let mut head = Vec::new();
                    let mut chunk = [0_u8; 1024];
                    loop {
                        match connection.read(&mut chunk) {
                            Ok(0) => {
                                if head.is_empty() {
                                    drop(sender.send(Seen::ClosedIdle));
                                }
                                return;
                            }
                            Ok(read) => {
                                head.extend_from_slice(&chunk[..read]);
                                if head.windows(4).any(|window| window == b"\r\n\r\n") {
                                    drop(connection.write_all(answer.as_bytes()));
                                    drop(sender.send(Seen::Answered));
                                    return;
                                }
                            }
                            Err(_) => return,
                        }
                    }
                });
            }
        });
        (address, seen)
    }

    /// The connector of an `https` endpoint whose first probe opening does not end until the test
    /// releases it, and then connects to `late`; every later opening connects to `fresh` at once.
    struct SlowFirstOpening {
        late: SocketAddr,
        fresh: SocketAddr,
        calls: AtomicUsize,
        released: Mutex<mpsc::Receiver<()>>,
    }

    impl PodConnector for SlowFirstOpening {
        fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
            Err(io::Error::other("no relay connection in this test"))
        }

        fn connect_probe(
            &self,
            _authority: &str,
            timeout: Duration,
        ) -> io::Result<Box<dyn RelayStream>> {
            let to = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                // A handshake still running at the probe's deadline.
                let _released = self.released.lock().unwrap().recv();
                self.late
            } else {
                self.fresh
            };
            let stream = TcpStream::connect(to)?;
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            Ok(Box::new(stream))
        }
    }

    struct Nothing;

    impl RelayTargets for Nothing {
        fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
            Err(TargetRefusal::Unavailable)
        }

        fn invalidate(&self, _alias: &str, _authority: &str) {}
    }

    fn targets(connector: Arc<SlowFirstOpening>) -> HostedTargets {
        let mut endpoints = BTreeMap::new();
        endpoints.insert(
            "m".to_owned(),
            HostedEndpoint::new("https://endpoint.invalid/v1", EndpointCredential::Anonymous)
                .unwrap(),
        );
        HostedTargets::new(endpoints, connector, Arc::new(Nothing)).with_probe_bounds(ProbeBounds {
            timeout: DEADLINE,
            reuse: Duration::ZERO,
        })
    }

    fn no_answer() -> ModelStatus {
        ModelStatus {
            phase: ModelPhase::Unreachable,
            sub_phase: Some("no-answer".to_owned()),
            profile: None,
        }
    }

    fn status_503() -> ModelStatus {
        ModelStatus {
            phase: ModelPhase::Unreachable,
            sub_phase: Some("status-503".to_owned()),
            profile: None,
        }
    }

    /// docs/gateway.md, "Bounds of the status probe": an opening left running at the deadline is
    /// left to end on its own, and "once it has ended, what it found is dropped". An `https` opening
    /// whose handshake ends after the deadline has opened a connection to the endpoint; once that
    /// opening has ended, the connection is closed, not held open until some later status read.
    #[test]
    fn route_status_adv2_a_connection_an_opening_made_after_the_deadline_is_not_held_open() {
        let (late, late_seen) = endpoint("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
        let (fresh, _fresh_seen) = endpoint("HTTP/1.1 503 Busy\r\ncontent-length: 0\r\n\r\n");
        let (release, released) = mpsc::channel();
        let connector = Arc::new(SlowFirstOpening {
            late,
            fresh,
            calls: AtomicUsize::new(0),
            released: Mutex::new(released),
        });
        let targets = targets(Arc::clone(&connector));
        assert_eq!(targets.status("m"), Some(no_answer()));
        release.send(()).unwrap();
        assert_eq!(
            late_seen.recv_timeout(Duration::from_secs(5)),
            Ok(Seen::Accepted),
            "the opening left running never connected"
        );
        // The opening has ended: it connected and handed its connection over. Nothing reads it.
        let after = late_seen.recv_timeout(Duration::from_secs(2));
        assert_eq!(
            after,
            Ok(Seen::ClosedIdle),
            "the connection opened after the deadline is still held open 2 s after its opening ended"
        );
        drop(targets);
    }

    /// docs/gateway.md, "Bounds of the status probe": once an opening left running has ended, what
    /// it found is dropped and the next probe opens afresh. The next status read answers from a
    /// fresh connection (503 here), never from the late one (whose endpoint would answer 200).
    #[test]
    fn route_status_adv2_the_next_probe_never_uses_a_late_opening() {
        let (late, late_seen) = endpoint("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
        let (fresh, fresh_seen) = endpoint("HTTP/1.1 503 Busy\r\ncontent-length: 0\r\n\r\n");
        let (release, released) = mpsc::channel();
        let connector = Arc::new(SlowFirstOpening {
            late,
            fresh,
            calls: AtomicUsize::new(0),
            released: Mutex::new(released),
        });
        let targets = targets(Arc::clone(&connector));
        assert_eq!(targets.status("m"), Some(no_answer()));
        release.send(()).unwrap();
        assert_eq!(
            late_seen.recv_timeout(Duration::from_secs(5)),
            Ok(Seen::Accepted)
        );
        assert_eq!(targets.status("m"), Some(status_503()));
        assert_eq!(connector.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            fresh_seen.recv_timeout(Duration::from_secs(1)),
            Ok(Seen::Accepted)
        );
        assert_eq!(
            late_seen.recv_timeout(Duration::from_secs(1)),
            Ok(Seen::ClosedIdle),
            "the late connection was sent the request"
        );
        drop(targets);
    }

    /// docs/gateway.md, "Bounds of the status probe": an `https` opening that does not end is
    /// `no-answer` at the deadline, and while it runs the next read starts no second one and is
    /// `no-answer` at its own deadline.
    #[test]
    fn route_status_adv2_an_https_opening_that_never_ends_is_no_answer_at_each_deadline() {
        let (late, _late_seen) = endpoint("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
        let (release, released) = mpsc::channel::<()>();
        let connector = Arc::new(SlowFirstOpening {
            late,
            fresh: late,
            calls: AtomicUsize::new(0),
            released: Mutex::new(released),
        });
        let targets = targets(Arc::clone(&connector));
        for read in 0..2 {
            let started = Instant::now();
            assert_eq!(targets.status("m"), Some(no_answer()), "{read}");
            let took = started.elapsed();
            assert!(took >= DEADLINE, "read {read} took {took:?}");
            assert!(
                took < DEADLINE + Duration::from_millis(500),
                "read {read} took {took:?}"
            );
        }
        assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
        drop(release);
        drop(targets);
    }
}

// --- Ollama models (story:ollama-route) --------------------------------------------------------
//
// Every Ollama here is `FakeOllama`, on loopback at port 0, answering in the forms an Ollama
// 0.40.18 server answers. No test sends a request to port 11434 or to any real Ollama, starts
// one, or pulls or loads a model.

/// The request lines the gateway may send an Ollama server: its two lists, the three wire paths,
/// and the messages count (the output clamp's, or a client's own).
const OLLAMA_ALLOWED: [&str; 6] = [
    "GET /api/tags",
    "GET /api/ps",
    "POST /v1/chat/completions",
    "POST /v1/responses",
    "POST /v1/messages",
    "POST /v1/messages/count_tokens",
];

/// The paths that change what an Ollama server stores.
const OLLAMA_STORING: [&str; 5] = [
    "/api/pull",
    "/api/create",
    "/api/copy",
    "/api/delete",
    "/api/push",
];

/// What the fake server holds: the models pulled, the models loaded, and how it answers.
#[derive(Default)]
struct OllamaState {
    pulled: Vec<String>,
    loaded: Vec<String>,
    /// Accept every connection and never answer.
    silent: bool,
    /// Milliseconds each answer's head is held back after the request is read.
    delay_ms: u64,
}

/// One request the fake read: its line (`<method> <path>`), its head lines but the first, and
/// its body.
#[derive(Clone, Debug)]
struct OllamaSeen {
    line: String,
    head: Vec<String>,
    body: String,
}

/// A fake Ollama server on loopback.
struct FakeOllama {
    address: SocketAddr,
    state: Arc<Mutex<OllamaState>>,
    seen: Arc<Mutex<Vec<OllamaSeen>>>,
}

impl FakeOllama {
    fn start(pulled: &[&str], loaded: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(OllamaState {
            pulled: pulled.iter().map(|name| (*name).to_owned()).collect(),
            loaded: loaded.iter().map(|name| (*name).to_owned()).collect(),
            ..OllamaState::default()
        }));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (serving, recording) = (Arc::clone(&state), Arc::clone(&seen));
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(connection) = connection else {
                    return;
                };
                let (state, seen) = (Arc::clone(&serving), Arc::clone(&recording));
                thread::spawn(move || ollama_answer(connection, &state, &seen));
            }
        });
        Self {
            address,
            state,
            seen,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn seen(&self) -> Vec<OllamaSeen> {
        self.seen.lock().unwrap().clone()
    }

    fn lines(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.line).collect()
    }

    /// Every request to a path that changes what the server stores.
    fn storing(&self) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|line| {
                let path = line.split(' ').nth(1).unwrap_or_default();
                OLLAMA_STORING
                    .iter()
                    .any(|storing| path.split('?').next() == Some(*storing))
            })
            .collect()
    }

    fn edit(&self, edit: impl FnOnce(&mut OllamaState)) {
        edit(&mut self.state.lock().unwrap());
    }
}

/// The JSON of one list entry, as `GET /api/tags` and `GET /api/ps` write it.
fn ollama_entry(name: &str) -> String {
    format!(
        "{{\"name\":\"{name}\",\"model\":\"{name}\",\"modified_at\":\"2026-11-25T10:00:00.000000000+01:00\",\"size\":986061892,\"digest\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"details\":{{\"format\":\"gguf\",\"family\":\"qwen2\"}},\"capabilities\":[\"completion\",\"tools\"]}}"
    )
}

fn ollama_list(names: &[String]) -> String {
    let entries: Vec<String> = names.iter().map(|name| ollama_entry(name)).collect();
    format!("{{\"models\":[{}]}}", entries.join(","))
}

/// The value of the first `"model":"…"` in `body`.
fn named_model(body: &str) -> String {
    body.split_once("\"model\":\"")
        .and_then(|(_, rest)| rest.split('"').next())
        .unwrap_or_default()
        .to_owned()
}

/// The `(input, output)` tokens the fake reports on each wire path.
fn ollama_usage(path: &str) -> (u64, u64) {
    match path {
        "/v1/chat/completions" => (11, 7),
        "/v1/responses" => (13, 5),
        _ => (17, 3),
    }
}

fn ollama_answer(
    mut connection: TcpStream,
    state: &Mutex<OllamaState>,
    seen: &Mutex<Vec<OllamaSeen>>,
) {
    connection.set_read_timeout(Some(WAIT)).unwrap();
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(at) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let head_lines: Vec<String> = lines.map(str::to_owned).collect();
    let length: usize = head_lines
        .iter()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0);
    let mut body = raw[head_end + 4..].to_vec();
    while body.len() < length {
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let mut parts = request_line.split(' ');
    let (method, path) = (
        parts.next().unwrap_or_default().to_owned(),
        parts.next().unwrap_or_default().to_owned(),
    );
    seen.lock().unwrap().push(OllamaSeen {
        line: format!("{method} {path}"),
        head: head_lines,
        body: body.clone(),
    });
    let (silent, delay) = {
        let state = state.lock().unwrap();
        (state.silent, state.delay_ms)
    };
    if silent {
        thread::sleep(WAIT);
        return;
    }
    thread::sleep(Duration::from_millis(delay));
    let answer = match (method.as_str(), path.as_str()) {
        ("GET", "/api/tags") => {
            // Written chunked, as Go writes a body it did not size, in two chunks.
            let list = ollama_list(&state.lock().unwrap().pulled);
            let (first, second) = list.split_at(list.len() / 2);
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json; charset=utf-8\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:x}\r\n{first}\r\n{:x}\r\n{second}\r\n0\r\n\r\n",
                first.len(),
                second.len()
            )
        }
        ("GET", "/api/ps") => ollama_json("200 OK", &ollama_list(&state.lock().unwrap().loaded)),
        ("POST", "/v1/chat/completions" | "/v1/responses" | "/v1/messages") => {
            ollama_call(&path, &named_model(&body), &mut state.lock().unwrap())
        }
        (_, path) if OLLAMA_STORING.contains(&path) => ollama_json(
            "500 Internal Server Error",
            "{\"error\":\"the fake Ollama changes nothing it stores\"}",
        ),
        _ => "HTTP/1.1 404 Not Found\r\ncontent-type: text/plain\r\ncontent-length: 18\r\nconnection: close\r\n\r\n404 page not found".to_owned(),
    };
    drop(connection.write_all(answer.as_bytes()));
}

/// A JSON answer of `status`, sized, as Ollama writes one.
fn ollama_json(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The fake's answer to a call on the wire `path` for `model`: Ollama's 404 for a model it has
/// not pulled, else the wire's answer with the tokens [`ollama_usage`] names. A call loads the
/// model.
fn ollama_call(path: &str, model: &str, state: &mut OllamaState) -> String {
    if !state.pulled.iter().any(|pulled| pulled == model) {
        return ollama_json(
            "404 Not Found",
            &format!(
                "{{\"error\":{{\"message\":\"model '{model}' not found\",\"type\":\"not_found_error\",\"param\":null,\"code\":null}}}}"
            ),
        );
    }
    if !state.loaded.iter().any(|loaded| loaded == model) {
        state.loaded.push(model.to_owned());
    }
    let (input, output) = ollama_usage(path);
    let body = match path {
        "/v1/chat/completions" => format!(
            "{{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion\",\"created\":1800000000,\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"message\":{{\"role\":\"assistant\",\"content\":\"hi\"}},\"finish_reason\":\"stop\"}}],\"usage\":{{\"prompt_tokens\":{input},\"completion_tokens\":{output},\"total_tokens\":{}}}}}",
            input + output
        ),
        "/v1/responses" => format!(
            "{{\"id\":\"resp_1\",\"object\":\"response\",\"created_at\":1800000000,\"model\":\"{model}\",\"status\":\"completed\",\"output\":[{{\"type\":\"message\",\"id\":\"msg_1\",\"status\":\"completed\",\"role\":\"assistant\",\"content\":[{{\"type\":\"output_text\",\"text\":\"hi\",\"annotations\":[]}}]}}],\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"total_tokens\":{}}}}}",
            input + output
        ),
        _ => format!(
            "{{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"{model}\",\"content\":[{{\"type\":\"text\",\"text\":\"hi\"}}],\"stop_reason\":\"end_turn\",\"stop_sequence\":null,\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output}}}}}"
        ),
    };
    ollama_json("200 OK", &body)
}

/// A source with one Ollama model `local` asking for `model` at `base_url`, within `bounds`.
fn ollama_targets(base_url: &str, model: &str, bounds: ProbeBounds) -> HostedTargets {
    HostedTargets::new(
        BTreeMap::from([(
            "local".to_owned(),
            HostedEndpoint::new(base_url, EndpointCredential::Anonymous)
                .unwrap()
                .with_ollama(model),
        )]),
        Arc::new(Loopback(silent_endpoint())),
        Arc::new(Nothing),
    )
    .with_probe_bounds(bounds)
}

/// Bounds that reuse nothing, so every read asks the server.
fn every_read(timeout: Duration) -> ProbeBounds {
    ProbeBounds {
        timeout,
        reuse: Duration::ZERO,
    }
}

fn phase(phase: ModelPhase, sub_phase: Option<&str>) -> ModelStatus {
    ModelStatus {
        phase,
        sub_phase: sub_phase.map(str::to_owned),
        profile: None,
    }
}

/// One case of a status read: the model asked for, the models pulled, the models loaded, the
/// status, and the request lines the server sees.
type OllamaCase = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    ModelStatus,
    &'static [&'static str],
);

/// Acceptance 3: the status is `GET /api/tags`, then `GET /api/ps` only when the model is
/// listed: not listed is `unreachable` `model-not-pulled`, listed and not loaded `stopped`
/// `not-loaded`, loaded `ready`, and no server `unreachable` `no-answer`. A name without a tag
/// matches `<name>:latest` only, a name with a tag itself only. Each request carries `host`,
/// `accept` and `connection: close` and no credential, and a read sends nothing else.
#[test]
fn ollama_status_is_read_from_tags_then_ps_and_sends_nothing_else() {
    let cases: [OllamaCase; 7] = [
        (
            "qwen2.5:1.5b",
            &["llama3.2:latest"],
            &[],
            phase(ModelPhase::Unreachable, Some("model-not-pulled")),
            &["GET /api/tags"],
        ),
        (
            "qwen2.5:1.5b",
            &["llama3.2:latest", "qwen2.5:1.5b"],
            &[],
            phase(ModelPhase::Stopped, Some("not-loaded")),
            &["GET /api/tags", "GET /api/ps"],
        ),
        (
            "qwen2.5:1.5b",
            &["qwen2.5:1.5b"],
            &["qwen2.5:1.5b"],
            phase(ModelPhase::Ready, None),
            &["GET /api/tags", "GET /api/ps"],
        ),
        (
            "qwen2.5",
            &["qwen2.5:latest"],
            &["qwen2.5:latest"],
            phase(ModelPhase::Ready, None),
            &["GET /api/tags", "GET /api/ps"],
        ),
        (
            "qwen2.5",
            &["qwen2.5:1.5b"],
            &[],
            phase(ModelPhase::Unreachable, Some("model-not-pulled")),
            &["GET /api/tags"],
        ),
        (
            "qwen2.5:latest",
            &["qwen2.5:1.5b"],
            &[],
            phase(ModelPhase::Unreachable, Some("model-not-pulled")),
            &["GET /api/tags"],
        ),
        (
            "library/qwen2.5",
            &["library/qwen2.5:latest"],
            &[],
            phase(ModelPhase::Stopped, Some("not-loaded")),
            &["GET /api/tags", "GET /api/ps"],
        ),
    ];
    for (model, pulled, loaded, expected, lines) in cases {
        let ollama = FakeOllama::start(pulled, loaded);
        let targets = ollama_targets(&ollama.base_url(), model, every_read(WAIT));
        assert_eq!(
            targets.status("local"),
            Some(expected),
            "{model} {pulled:?}"
        );
        assert_eq!(ollama.lines(), lines, "{model} {pulled:?}");
        for seen in ollama.seen() {
            assert_eq!(
                seen.head,
                [
                    format!("host: {}", ollama.address),
                    "accept: application/json".to_owned(),
                    "connection: close".to_owned()
                ],
                "{model}"
            );
            assert!(seen.body.is_empty());
        }
        assert!(ollama.storing().is_empty());
    }
    // No server: nothing listens at the address.
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let targets = ollama_targets(
        &format!("http://{address}"),
        "qwen2.5:1.5b",
        every_read(WAIT),
    );
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Unreachable, Some("no-answer")))
    );
}

/// Acceptance 3: an answer that is not Ollama's list is no answer, and a non-2xx status says
/// which; a status read of an Ollama model never asks `GET /v1/models`.
#[test]
fn ollama_an_answer_that_is_not_a_list_is_no_answer_and_a_failed_status_is_named() {
    for (answer, expected) in [
        (
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n[]",
            phase(ModelPhase::Unreachable, Some("no-answer")),
        ),
        (
            "HTTP/1.1 200 OK\r\ncontent-length: 9\r\nconnection: close\r\n\r\nnot json!",
            phase(ModelPhase::Unreachable, Some("no-answer")),
        ),
        (
            "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            phase(ModelPhase::Unreachable, Some("status-503")),
        ),
        (
            "HTTP/1.1 404 Not Found\r\ncontent-type: text/plain\r\ncontent-length: 18\r\nconnection: close\r\n\r\n404 page not found",
            phase(ModelPhase::Unreachable, Some("status-404")),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection.set_read_timeout(Some(WAIT)).unwrap();
            let mut seen = [0_u8; 4096];
            let read = connection.read(&mut seen).unwrap_or(0);
            drop(connection.write_all(answer.as_bytes()));
            String::from_utf8_lossy(&seen[..read]).into_owned()
        });
        let targets = ollama_targets(
            &format!("http://{address}"),
            "qwen2.5:1.5b",
            every_read(WAIT),
        );
        assert_eq!(targets.status("local"), Some(expected), "{answer}");
        assert!(
            server
                .join()
                .unwrap()
                .starts_with("GET /api/tags HTTP/1.1\r\n"),
            "{answer}"
        );
    }
}

/// Acceptance 3 inside the probe's bounds: both requests end within one deadline (each answer
/// held 400 ms past a 600 ms deadline is `no-answer` at the deadline, not after 800 ms), a
/// silent server is `no-answer` at the deadline, and a result is reused until the reuse time
/// has passed, one read of the lists for any number of status reads inside it.
#[test]
fn ollama_a_status_read_keeps_the_probe_bounds_and_is_reused() {
    let deadline = Duration::from_millis(600);
    let slow = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    slow.edit(|state| state.delay_ms = 400);
    let targets = ollama_targets(&slow.base_url(), "qwen2.5:1.5b", every_read(deadline));
    let started = Instant::now();
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Unreachable, Some("no-answer")))
    );
    let took = started.elapsed();
    assert!(
        took >= deadline && took < deadline + Duration::from_millis(500),
        "{took:?}"
    );
    assert_eq!(slow.lines(), ["GET /api/tags", "GET /api/ps"]);
    let silent = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    silent.edit(|state| state.silent = true);
    let targets = ollama_targets(&silent.base_url(), "qwen2.5:1.5b", every_read(deadline));
    let started = Instant::now();
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Unreachable, Some("no-answer")))
    );
    assert!(started.elapsed() < deadline + Duration::from_millis(500));
    // Reuse, on a clock the test moves.
    let ollama = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let clock = Arc::clone(&now);
    let targets = ollama_targets(&ollama.base_url(), "qwen2.5:1.5b", ProbeBounds::default())
        .with_probe_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
    let not_loaded = Some(phase(ModelPhase::Stopped, Some("not-loaded")));
    assert_eq!(targets.status("local"), not_loaded);
    now.store(1_000 + 9_999, Ordering::SeqCst);
    assert_eq!(targets.status("local"), not_loaded);
    assert_eq!(ollama.lines(), ["GET /api/tags", "GET /api/ps"]);
    now.store(1_000 + 10_000, Ordering::SeqCst);
    assert_eq!(targets.status("local"), not_loaded);
    assert_eq!(
        ollama.lines(),
        [
            "GET /api/tags",
            "GET /api/ps",
            "GET /api/tags",
            "GET /api/ps"
        ]
    );
    assert!(
        targets.wake("local").is_none(),
        "an Ollama model has no wake"
    );
}

/// Acceptance 4: `acquire` reads the same bounded and reused status. A model the server
/// answered without listing is refused `NotPulled`; a pulled model, loaded or not, is handed
/// out, and so is a model whose server does not answer (the call goes on). A model deleted
/// inside the reuse window is still handed out: the known limit.
#[test]
fn ollama_acquire_refuses_only_a_model_the_server_answered_without() {
    let missing = FakeOllama::start(&["llama3.2:latest"], &[]);
    let targets = ollama_targets(&missing.base_url(), "qwen2.5:1.5b", ProbeBounds::default());
    assert_eq!(
        targets.acquire("local").err(),
        Some(TargetRefusal::NotPulled)
    );
    // The status read after it is the same read, reused.
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Unreachable, Some("model-not-pulled")))
    );
    assert_eq!(missing.lines(), ["GET /api/tags"]);
    for loaded in [&[][..], &["qwen2.5:1.5b"][..]] {
        let pulled = FakeOllama::start(&["qwen2.5:1.5b"], loaded);
        let targets = ollama_targets(&pulled.base_url(), "qwen2.5:1.5b", ProbeBounds::default());
        let target = targets.acquire("local").unwrap();
        assert_eq!(target.authority(), pulled.address.to_string());
        assert_eq!(pulled.lines(), ["GET /api/tags", "GET /api/ps"]);
    }
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = closed.local_addr().unwrap();
    drop(closed);
    let targets = ollama_targets(
        &format!("http://{address}"),
        "qwen2.5:1.5b",
        every_read(WAIT),
    );
    assert!(targets.acquire("local").is_ok(), "no answer is no refusal");
    let deleted = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    let targets = ollama_targets(&deleted.base_url(), "qwen2.5:1.5b", ProbeBounds::default());
    assert!(targets.acquire("local").is_ok());
    deleted.edit(|state| state.pulled.clear());
    assert!(
        targets.acquire("local").is_ok(),
        "inside the reuse window the earlier read stands"
    );
    for fake in [&missing, &deleted] {
        assert!(fake.storing().is_empty());
    }
}

/// The deployment document of the support crate with the Ollama models `local`, pulled, and
/// `missing`, not pulled, on `ollama`.
fn ollama_deployment(fixture: &Fixture, ollama: &FakeOllama) -> Deployment {
    deployment(fixture, |text| {
        format!(
            "{text}\n[hosted.local]\nkind = \"ollama\"\nbase_url = \"{0}\"\nupstream_model = \"qwen2.5:1.5b\"\n\n\
             [hosted.missing]\nkind = \"ollama\"\nbase_url = \"{0}\"\nupstream_model = \"llama3.2\"\n",
            ollama.base_url()
        )
    })
}

/// What the binary's composition answers to one exercise of its Ollama models: a status read, a
/// wake, the list, a call on each wire to the pulled model and one to the model not pulled.
struct Exercised {
    status: String,
    wake: String,
    list: String,
    calls: Vec<String>,
    unpulled: String,
}

/// `name` names the fixture directory, one per test: two tests sharing one would remove each
/// other's files.
fn exercise(ollama: &FakeOllama, name: &str) -> Exercised {
    let fixture = Fixture::new(name);
    let deployment = ollama_deployment(&fixture, ollama);
    let mut pod = Pod::start(0, "{}");
    let runpod = EmulatedRunpod::new();
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback(pod.address)),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let address = running.local_addr();
    let status = send(address, "GET", "/v1/models/local/status", "");
    let wake = send(address, "POST", "/v1/models/local/wake", "");
    let list = send(address, "GET", "/v1/status", "");
    let calls = [
        ("/v1/chat/completions", "{\"model\":\"local\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"),
        ("/v1/responses", "{\"model\":\"local\",\"input\":\"hi\"}"),
        ("/v1/messages", "{\"model\":\"local\",\"max_tokens\":16,\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"),
    ]
    .into_iter()
    .map(|(path, body)| send(address, "POST", path, body))
    .collect();
    let unpulled = send(
        address,
        "POST",
        "/v1/chat/completions",
        "{\"model\":\"missing\",\"messages\":[]}",
    );
    running.shutdown();
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(pod.served(), 0);
    Exercised {
        status,
        wake,
        list,
        calls,
        unpulled,
    }
}

/// Acceptance 2, 3 and 4 in the binary's composition: the status is read from the server's
/// lists, the wake is `path-unknown`, the list names kind `ollama`, each wire is relayed to
/// Ollama's own path under the Ollama model name, and a call to a model not pulled is
/// `model-not-pulled`, naming its alias. Every request line the server saw is in the allowed
/// set: its two lists and the wire paths.
#[test]
fn ollama_the_binary_sends_the_server_only_its_lists_and_the_wire_calls() {
    let ollama = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    let exercised = exercise(&ollama, "ollama-allowed-lines");
    assert!(
        exercised.status.starts_with("HTTP/1.1 200 "),
        "{}",
        exercised.status
    );
    assert_eq!(
        body(&exercised.status),
        "{\"model\":\"local\",\"phase\":\"stopped\",\"sub_phase\":\"not-loaded\"}"
    );
    assert!(
        exercised.wake.starts_with("HTTP/1.1 404 "),
        "{}",
        exercised.wake
    );
    assert!(exercised.wake.contains("\"code\":\"path-unknown\""));
    let wires = "\"wires\":[\"chat\",\"responses\",\"messages\"]";
    assert!(
        body(&exercised.list).starts_with(&format!(
            "{{\"format\":\"llm-gateway.status/1\",\"models\":[\
             {{\"model\":\"local\",\"kind\":\"ollama\",{wires},\"phase\":\"stopped\",\"sub_phase\":\"not-loaded\",\"callable\":true}},\
             {{\"model\":\"missing\",\"kind\":\"ollama\",{wires},\"phase\":\"unreachable\",\"sub_phase\":\"model-not-pulled\",\"callable\":false}},"
        )),
        "{}",
        exercised.list
    );
    for call in &exercised.calls {
        assert!(call.starts_with("HTTP/1.1 200 "), "{call}");
        assert!(call.contains("\"model\":\"qwen2.5:1.5b\""), "{call}");
    }
    assert!(
        exercised.unpulled.starts_with("HTTP/1.1 503 "),
        "{}",
        exercised.unpulled
    );
    assert!(
        !exercised.unpulled.contains("retry-after"),
        "{}",
        exercised.unpulled
    );
    assert!(
        body(&exercised.unpulled).ends_with(": missing\"}}"),
        "{}",
        exercised.unpulled
    );
    let lines = ollama.lines();
    let outside: Vec<&String> = lines
        .iter()
        .filter(|line| !OLLAMA_ALLOWED.contains(&line.as_str()))
        .collect();
    assert!(
        outside.is_empty(),
        "outside the allowed set: {outside:?} of {lines:?}"
    );
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        let relayed: Vec<OllamaSeen> = ollama
            .seen()
            .into_iter()
            .filter(|seen| seen.line == format!("POST {path}"))
            .collect();
        assert_eq!(relayed.len(), 1, "{path}: {lines:?}");
        assert_eq!(named_model(&relayed[0].body), "qwen2.5:1.5b", "{path}");
        assert!(
            !relayed[0]
                .head
                .iter()
                .any(|line| line.to_ascii_lowercase().starts_with("authorization")),
            "{path}: the owner's credential reached the server"
        );
    }
    // The model not pulled was refused before any call reached the server.
    assert!(
        !ollama
            .seen()
            .iter()
            .any(|seen| named_model(&seen.body) == "llama3.2")
    );
}

/// Acceptance 4: no request the gateway sends an Ollama server pulls, creates, copies, deletes
/// or pushes a model, over a status read, a wake, the list, a call on each wire and a call to a
/// model not pulled. Any such request fails this test.
#[test]
fn ollama_the_gateway_never_asks_the_server_to_change_what_it_stores() {
    let ollama = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    let _exercised = exercise(&ollama, "ollama-never-storing");
    assert!(!ollama.lines().is_empty(), "the server was asked nothing");
    assert_eq!(ollama.storing(), Vec::<String>::new());
}

/// Every usage record the relay hands out, whole.
#[derive(Default)]
struct KeptRecords(Mutex<Vec<llm_gateway::UsageRecord>>);

impl llm_gateway::UsageRecords for KeptRecords {
    fn record(&self, record: &llm_gateway::UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

/// Acceptance 5 and "a pulled model that is not loaded is callable": the binary's hosted target
/// source serves an Ollama model not yet loaded on each wire; each call is relayed, the first
/// loads the model, and each usage record carries the tokens the server reported on that wire,
/// as a hosted model's does.
#[test]
fn ollama_each_wire_is_relayed_and_its_usage_record_carries_the_servers_tokens() {
    let ollama = FakeOllama::start(&["qwen2.5:1.5b"], &[]);
    let targets = ollama_targets(&ollama.base_url(), "qwen2.5:1.5b", every_read(WAIT));
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Stopped, Some("not-loaded")))
    );
    let model = RelayModel::new(
        Label::new("local").unwrap(),
        Label::new("qwen2.5:1.5b").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
    .with_kind(ModelKind::Ollama);
    let records = Arc::new(KeptRecords::default());
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap())
            .unwrap(),
    );
    let targets = Arc::new(targets);
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        Relay::new(vec![model], Arc::clone(&targets) as Arc<dyn RelayTargets>)
            .unwrap()
            .with_records(Arc::clone(&records) as Arc<dyn llm_gateway::UsageRecords>),
    )
    .unwrap();
    handle.mark_ready();
    let paths = [
        ("/v1/chat/completions", Wire::Chat),
        ("/v1/responses", Wire::Responses),
        ("/v1/messages", Wire::Messages),
    ];
    for (path, _) in paths {
        let answer = send(
            handle.local_addr(),
            "POST",
            path,
            "{\"model\":\"local\",\"max_tokens\":16,\"messages\":[]}",
        );
        assert!(answer.starts_with("HTTP/1.1 200 "), "{path}: {answer}");
    }
    handle.shutdown();
    let records = records.0.lock().unwrap();
    let seen: Vec<_> = records
        .iter()
        .map(|record| {
            (
                record.model.clone(),
                record.wire,
                record.target_status,
                record.input_tokens,
                record.output_tokens,
            )
        })
        .collect();
    let expected: Vec<_> = paths
        .iter()
        .map(|(path, wire)| {
            let (input, output) = ollama_usage(path);
            (
                Some("local".to_owned()),
                *wire,
                Some(200),
                Some(input),
                Some(output),
            )
        })
        .collect();
    assert_eq!(seen, expected);
    assert_eq!(
        targets.status("local"),
        Some(phase(ModelPhase::Ready, None)),
        "the first call loaded the model"
    );
    assert!(ollama.storing().is_empty());
}
