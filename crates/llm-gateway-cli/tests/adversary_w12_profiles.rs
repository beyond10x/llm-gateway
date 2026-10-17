//! Adversarial pass over story:model-profile-fallback: the output clamp of a call served by a
//! fallback profile. Composed in process through `start_relaying` with `EmulatedRunpod` and a
//! loopback pod; nothing leaves the loopback interface and no Runpod API is called.

mod support;

use llm_gateway::RelayStream;
use llm_gateway_cli::{PodConnector, load, start_relaying};
use llm_runpod::{EmulatedRunpod, ManualClock};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use support::{Fixture, OWNER_SECRET, document, mutate, with_vllm_key};

const WAIT: Duration = Duration::from_secs(10);

/// A loopback pod answering each of `answers` connections with `body`, recording each request
/// as `<request line> <body>`.
struct Pod {
    address: SocketAddr,
    received: Arc<Mutex<Vec<String>>>,
    _thread: JoinHandle<()>,
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
                let raw = read_request(&mut connection);
                let text = String::from_utf8_lossy(&raw).into_owned();
                let line = text.split("\r\n").next().unwrap_or_default().to_owned();
                let request_body = text.split_once("\r\n\r\n").map_or("", |(_, body)| body);
                recording
                    .lock()
                    .unwrap()
                    .push(format!("{line} {request_body}"));
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
            _thread: thread,
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

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

struct Loopback {
    pod: SocketAddr,
}

impl PodConnector for Loopback {
    fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(self.pod)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

fn post(address: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut answer = String::new();
    drop(stream.read_to_string(&mut answer));
    answer
}

/// The output clamp (`docs/gateway.md` step 7) keeps a messages request within the window of
/// the server that answers it, because vLLM answers a `max_tokens` past `max_model_len - n` with
/// an error. A profiled model's first profile declares 2048, its fallback 1024; the first is
/// out of capacity, so the pod that serves runs `--max-model-len 1024`. With 1000 input tokens
/// counted, the relayed `max_tokens` must be 24 (1024 - 1000), as
/// `clamp_a_runpod_model_is_clamped_to_its_max_model_len_a_hosted_one_to_its_context_window`
/// pins for the one-profile model. The relay takes its window from the first profile only
/// (`relay_models` in `crates/llm-gateway-cli/src/relaying.rs`, `model.vllm.max_model_len`), so
/// it sends 1048. Reached by the fallback block in `docs/model-profiles.md` (h100-64k, then
/// l40s-32k) whenever a 64k profile is out of stock, with any client that sends `max_tokens`.
#[test]
fn adv_profiles_a_call_served_by_a_fallback_profile_is_clamped_to_that_profiles_max_model_len() {
    const COUNT: &str = "{\"input_tokens\":1000}";
    let fixture = Fixture::new("adv-profiles-clamp");
    let secret = fixture.owner_secret();
    let key = fixture.vllm_key();
    let text = with_vllm_key(&document("127.0.0.1:0", &secret), &key);
    let text = mutate(
        &text,
        "wires = [\"chat\", \"responses\"]",
        "wires = [\"chat\", \"responses\", \"messages\"]",
    );
    let text = mutate(
        &text,
        "gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n",
        "",
    );
    let text = format!(
        "{text}\n[[models.small.profiles]]\nname = \"h100-2k\"\ngpu_types = [\"NVIDIA H100 NVL\"]\nmax_model_len = 2048\n\n[[models.small.profiles]]\nname = \"l40s-1k\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n"
    );
    let deployment = load(&fixture.config(&text)).unwrap();
    let pod = Pod::start(2, COUNT);
    let runpod = EmulatedRunpod::new();
    runpod.no_capacity_gpu("NVIDIA H100 NVL");
    let running = start_relaying(
        &deployment,
        runpod.clone(),
        Arc::new(Loopback { pod: pod.address }),
        Arc::new(ManualClock::new(1_000)),
    )
    .unwrap();
    let answer = post(
        running.local_addr(),
        "/v1/messages",
        "{\"model\":\"small\",\"max_tokens\":32000,\"messages\":[]}",
    );
    running.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let gpus: Vec<String> = runpod
        .requests()
        .iter()
        .map(|request| request.gpu_type.clone())
        .collect();
    assert_eq!(
        gpus,
        ["NVIDIA H100 NVL", "NVIDIA L40S"],
        "the fallback served"
    );
    let entrypoint = runpod.requests().pop().unwrap().docker_entrypoint;
    let at = entrypoint
        .iter()
        .position(|arg| arg == "--max-model-len")
        .unwrap();
    assert_eq!(entrypoint[at + 1], "1024", "the serving pod's window");
    let received = pod.received.lock().unwrap().clone();
    assert_eq!(
        received.last().map(String::as_str),
        Some("POST /v1/messages HTTP/1.1 {\"model\":\"small\",\"max_tokens\":24,\"messages\":[]}"),
        "every request the pod read: {received:?}"
    );
}
