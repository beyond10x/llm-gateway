//! The owner's lifecycle routes in the binary's composition (story:model-wake-and-status):
//! `POST /v1/models/{alias}/wake` starts a Runpod model's pod through the same pool path a
//! held request takes, so the spend check and the profile fallback apply, and
//! `GET /v1/models/{alias}/status` reads the phase from the pool without a provider call.
//! Composed in process through the `start_relaying` seam with `EmulatedRunpod` and a loopback
//! pod; no Runpod API is called and nothing leaves the loopback interface.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{Deployment, PodConnector, load, start_relaying};
use llm_runpod::{AccountSpend, EmulatedRunpod, ManualClock};
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
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

#[test]
fn a_hosted_model_has_no_lifecycle_and_is_path_unknown_beside_a_pod_model() {
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
    for answer in [&wake, &status] {
        assert!(answer.starts_with("HTTP/1.1 404 "), "{answer}");
        assert!(answer.contains("\"code\":\"path-unknown\""), "{answer}");
    }
    assert!(pod_status.starts_with("HTTP/1.1 200 "), "{pod_status}");
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(pod.served(), 0);
}
