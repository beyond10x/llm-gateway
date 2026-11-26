//! story:runpod-rest-host: the composition the shipped binary runs for a provider with a keychain
//! `api_key` (`start_connected`), against a fake Runpod on loopback that serves both hosts: the
//! management API (REST API v2 and the GraphQL fallback) and the inference route of each pod
//! (the proxy). The test's pod connector reaches the fake for every pod host and answers a host
//! it does not know as a resolver does, so no name is resolved and nothing leaves the loopback
//! interface. No pod is started anywhere, no keychain is read (a fixed in-memory one is
//! installed) and every key is a test literal.

use llm_gateway::{RelayStream, StreamAssembler, Wire};
use llm_gateway_cli::{
    EntryValue, EventAssembler, KeychainRef, LookupFailure, MAX_ASSEMBLED_BYTES, PodConnector,
    SecretSource, install_secret_source, parse, start_connected,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, Once},
    thread,
    time::{Duration, Instant},
};

const RUNPOD_KEY: &str = "rpa_runpod-key-from-the-test-keychain";
const VLLM_KEY: &str = "vllm-key-from-the-test-keychain";
const WAIT: Duration = Duration::from_secs(20);
/// The longest the fake proxy lets its origin stay silent before it answers `524`, as Runpod's
/// proxy does after its own bound.
const IDLE: Duration = Duration::from_millis(400);
/// The gap between two events of a streamed answer: under `IDLE`, while the whole answer runs
/// longer than it.
const GAP: Duration = Duration::from_millis(150);

struct FixedKeychain;

impl SecretSource for FixedKeychain {
    fn lookup(&self, reference: &KeychainRef) -> Result<EntryValue, LookupFailure> {
        let value = match (reference.service(), reference.account()) {
            ("test-gateway", "runpod") => RUNPOD_KEY,
            ("test-gateway", "vllm") => VLLM_KEY,
            _ => return Err(LookupFailure::Missing),
        };
        Ok(EntryValue::new(value.as_bytes().to_vec()))
    }
}

fn keychain() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| assert!(install_secret_source(Arc::new(FixedKeychain))));
}

/// How the fake's management API answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Management {
    Up,
    Status(u16),
    Closed,
}

/// One request the fake received.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub(crate) host: String,
    pub(crate) line: String,
    pub(crate) head: String,
    body: Vec<u8>,
}

impl Seen {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }
}

struct State {
    management: Management,
    pods: Vec<Value>,
    created: u32,
    seen: Vec<Seen>,
    /// When each event of the last streamed answer was written, from its first.
    written: Vec<Duration>,
}

/// The fake Runpod: both hosts on one loopback listener.
pub(crate) struct Fake {
    pub(crate) address: SocketAddr,
    state: Arc<Mutex<State>>,
}

impl Fake {
    pub(crate) fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(State {
            management: Management::Up,
            pods: Vec::new(),
            created: 0,
            seen: Vec::new(),
            written: Vec::new(),
        }));
        let shared = Arc::clone(&state);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(connection) = connection else { return };
                let state = Arc::clone(&shared);
                thread::spawn(move || serve(connection, &state));
            }
        });
        Self { address, state }
    }

    fn management(&self, mode: Management) {
        self.state.lock().unwrap().management = mode;
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    fn lines(&self) -> Vec<String> {
        self.seen().into_iter().map(|seen| seen.line).collect()
    }

    fn written(&self) -> Vec<Duration> {
        self.state.lock().unwrap().written.clone()
    }

    pub(crate) fn management_url(&self) -> String {
        format!("http://{}/v2", self.address)
    }
}

fn read_request(connection: &mut TcpStream) -> Option<Seen> {
    connection.set_read_timeout(Some(WAIT)).ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 16_384];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let lower = head.to_ascii_lowercase();
    let wanted: usize = lower
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < end + 4 + wanted {
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
    let host = lower
        .split("\r\n")
        .find_map(|line| line.strip_prefix("host:"))
        .map(|host| host.trim().to_owned())
        .unwrap_or_default();
    Some(Seen {
        host,
        line: head.split("\r\n").next().unwrap_or_default().to_owned(),
        head: lower,
        body: buffer[end + 4..(end + 4 + wanted).min(buffer.len())].to_vec(),
    })
}

fn json_answer(status: u16, body: &Value) -> Vec<u8> {
    let body = body.to_string();
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn chunk(piece: &str) -> Vec<u8> {
    format!("{:x}\r\n{piece}\r\n", piece.len()).into_bytes()
}

fn serve(mut connection: TcpStream, state: &Arc<Mutex<State>>) {
    let Some(seen) = read_request(&mut connection) else {
        return;
    };
    state.lock().unwrap().seen.push(seen.clone());
    let line = seen.line.clone();
    let management = line.contains(" /v2/") || line.starts_with("POST /graphql ");
    if management {
        let mode = state.lock().unwrap().management;
        match mode {
            Management::Up => {}
            Management::Status(status) => {
                drop(connection.write_all(&json_answer(status, &json!({"error": "down"}))));
                return;
            }
            Management::Closed => {
                drop(connection.shutdown(Shutdown::Both));
                return;
            }
        }
    }
    let answer = if line.starts_with("GET /v2/pods ") || line.starts_with("GET /v2/pods?") {
        let pods = state.lock().unwrap().pods.clone();
        json_answer(
            200,
            &json!({"pods": pods, "pagination": {"hasNextPage": false, "nextCursor": null}}),
        )
    } else if line.starts_with("POST /v2/pods ") {
        let body = seen.json();
        let mut held = state.lock().unwrap();
        held.created += 1;
        let pod = json!({
            "id": format!("pod{}", held.created),
            "name": body["name"],
            "image": body["image"],
            "desiredStatus": "RUNNING",
            "costPerHr": 0.79,
            "gpu": body["gpu"],
            "env": body["env"],
            "machine": {"secureCloud": true, "gpuTypeId": body["gpu"]["id"]},
            "lastStartedAt": "2026-11-26T08:00:00.000Z",
        });
        held.pods.push(pod.clone());
        json_answer(201, &pod)
    } else if let Some(id) = line
        .strip_prefix("DELETE /v2/pods/")
        .and_then(|rest| rest.split(' ').next())
    {
        state.lock().unwrap().pods.retain(|pod| pod["id"] != id);
        b"HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n".to_vec()
    } else if line.starts_with("GET /v2/billing/balance ") {
        json_answer(
            200,
            &json!({"balanceAmount": 25.5, "billingMode": "PREPAID"}),
        )
    } else if line.starts_with("GET /v2/billing/pods") {
        json_answer(200, &json!([]))
    } else if line.starts_with("POST /graphql ") {
        json_answer(
            200,
            &json!({"data": {"myself": {"currentSpendPerHr": 0.79}}}),
        )
    } else if line.starts_with("GET /v1/models ") {
        json_answer(
            200,
            &json!({"object": "list", "data": [{"id": "example/small-model"}]}),
        )
    } else if line.starts_with("POST /v1/messages/count_tokens ") {
        json_answer(200, &json!({"input_tokens": 5}))
    } else if line.starts_with("POST /v1/chat/completions ") {
        return proxy(&mut connection, &seen, state, CHAT_EVENTS);
    } else if line.starts_with("POST /v1/messages ") {
        return proxy(&mut connection, &seen, state, MESSAGES_EVENTS);
    } else {
        json_answer(404, &json!({"error": "no such path"}))
    };
    drop(connection.write_all(&answer));
}

/// The proxy in front of a pod: a call that asks for a stream gets its first event at once and
/// each next one `GAP` later; a call that does not is answered nothing for `IDLE` and then `524`,
/// as the proxy answers an origin that stayed silent past its bound. A call whose last message
/// says `cut` is closed after its first event.
fn proxy(connection: &mut TcpStream, seen: &Seen, state: &Arc<Mutex<State>>, events: &[&str]) {
    let body = seen.json();
    let cut = body.to_string().contains("\"cut\"");
    if body["stream"] != json!(true) {
        thread::sleep(IDLE);
        let text = "A timeout occurred";
        drop(write!(
            connection,
            "HTTP/1.1 524 \r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
            text.len()
        ));
        return;
    }
    let start = Instant::now();
    state.lock().unwrap().written.clear();
    let head = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";
    if connection.write_all(head).is_err() {
        return;
    }
    for (index, event) in events.iter().enumerate() {
        if index > 0 {
            thread::sleep(GAP);
        }
        if connection.write_all(&chunk(event)).is_err() {
            return;
        }
        state.lock().unwrap().written.push(start.elapsed());
        if cut {
            drop(connection.shutdown(Shutdown::Both));
            return;
        }
    }
    drop(connection.write_all(b"0\r\n\r\n"));
}

/// A chat answer, as vLLM streams one: role, reasoning, content in two pieces, a tool call in two
/// pieces, the finish, the usage chunk, `[DONE]`.
const CHAT_EVENTS: &[&str] = &[
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"logprobs\":null,\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"Thinking\"},\"logprobs\":null,\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"logprobs\":null,\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\", world\"},\"logprobs\":null,\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]},\"logprobs\":null,\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"x\\\"}\"}}]},\"logprobs\":null,\"finish_reason\":\"tool_calls\"}]}\n\n",
    "data: {\"id\":\"chatcmpl-7\",\"object\":\"chat.completion.chunk\",\"created\":1764140000,\"model\":\"example/small-model\",\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":7,\"total_tokens\":18}}\n\ndata: [DONE]\n\n",
];

/// The non-streamed chat answer of [`CHAT_EVENTS`].
fn chat_answer() -> Value {
    json!({
        "id": "chatcmpl-7",
        "object": "chat.completion",
        "created": 1_764_140_000,
        "model": "example/small-model",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello, world",
                "reasoning_content": "Thinking",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "lookup", "arguments": "{\"q\":\"x\"}"},
                }],
            },
            "logprobs": null,
            "finish_reason": "tool_calls",
        }],
        "usage": {"prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18},
    })
}

/// A messages answer as a stream: thinking with its signature, text in two pieces, a tool use
/// whose input arrives in two pieces, the stop and the usage.
const MESSAGES_EVENTS: &[&str] = &[
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"example/small-model\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":11,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Hmm\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"lookup\",\"input\":{}}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"q\\\":\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"x\\\"}\"}}\n\nevent: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":9}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
];

/// The non-streamed messages answer of [`MESSAGES_EVENTS`].
fn messages_answer() -> Value {
    json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "example/small-model",
        "content": [
            {"type": "thinking", "thinking": "Hmm", "signature": "sig"},
            {"type": "text", "text": "Hi there"},
            {"type": "tool_use", "id": "toolu_1", "name": "lookup", "input": {"q": "x"}},
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {"input_tokens": 11, "output_tokens": 9},
    })
}

/// The test's pod connector: every host it maps reaches the fake over plain TCP; every other host
/// fails as an unresolvable name does. It records each host it was asked for.
pub(crate) struct Pods {
    fake: SocketAddr,
    known: Vec<&'static str>,
    asked: Mutex<Vec<String>>,
}

impl Pods {
    pub(crate) fn new(fake: &Fake, known: Vec<&'static str>) -> Arc<Self> {
        Arc::new(Self {
            fake: fake.address,
            known,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn open(&self, authority: &str) -> io::Result<TcpStream> {
        self.asked.lock().unwrap().push(authority.to_owned());
        let host = authority.split(':').next().unwrap_or_default();
        if !self.known.iter().any(|known| host.ends_with(known)) {
            return Err(io::Error::other(
                "failed to lookup address information: Name or service not known",
            ));
        }
        let stream = TcpStream::connect(self.fake)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(stream)
    }

    fn asked(&self) -> BTreeSet<String> {
        self.asked.lock().unwrap().iter().cloned().collect()
    }
}

impl PodConnector for Pods {
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(self.open(authority)?))
    }

    fn connect_probe(
        &self,
        authority: &str,
        _timeout: Duration,
    ) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(self.open(authority)?))
    }
}

/// A gateway with no ingress credential, one Runpod provider with the keychain `api_key` and
/// the provider lines `lines`, and one model `small` on chat and messages.
fn document(lines: &str) -> String {
    format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\n\
         kind = \"runpod-vllm\"\napi_key = \"keychain:test-gateway/runpod\"\n{lines}\n\
         [models.small]\nprovider = \"runpod\"\nwires = [\"chat\", \"messages\"]\n\
         context_window = 65536\nhf_model = \"example/small-model\"\n\
         image = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\n\
         max_model_len = 1024\nvllm_api_key = \"keychain:test-gateway/vllm\"\n"
    )
}

/// The provider lines that point both hosts at the fake: the management API by its loopback
/// address, the proxy by a pod host the test connector maps to it.
pub(crate) fn both_at(fake: &Fake) -> String {
    format!(
        "management_url = \"{}\"\nproxy_url = \"https://{{pod_id}}-{{port}}.pods.fake.test\"\n",
        fake.management_url()
    )
}

fn send(address: SocketAddr, method: &str, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(50)))
        .unwrap();
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
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\n{length}\r\n{body}"
    )
    .unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

fn body(answer: &str) -> &str {
    answer.split_once("\r\n\r\n").map_or("", |(_, body)| body)
}

fn chat(prompt: &str, stream: bool) -> String {
    let stream = if stream { ",\"stream\":true" } else { "" };
    format!(
        "{{\"model\":\"small\",\"messages\":[{{\"role\":\"user\",\"content\":\"{prompt}\"}}]{stream}}}"
    )
}

/// Everything the binary's subscriber writes while `run` runs on this thread.
fn logged<T>(run: impl FnOnce() -> T) -> (T, String) {
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
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(llm_gateway_cli::log_filter(None))
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let value = tracing::subscriber::with_default(subscriber, run);
    let text = String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
    (value, text)
}

/// Acceptance 7: the defaults are the documented hosts, and the default proxy URL fills to
/// exactly the endpoint llm's Runpod description builds.
#[test]
fn runpod_host_the_defaults_are_the_documented_hosts() {
    let deployment = parse(&document("")).unwrap();
    let provider = &deployment.providers["runpod"];
    assert_eq!(provider.management_url, "https://api.runpod.io/v2");
    assert_eq!(
        provider.proxy_url.as_str(),
        "https://{pod_id}-{port}.proxy.runpod.net"
    );
    let described = llm_providers::descriptions::runpod()
        .inference_base_url("abc123")
        .unwrap();
    assert_eq!(provider.proxy_url.endpoint("abc123"), described.as_str());
    let deployment = parse(&document(
        "management_url = \"https://runpod.example.test/v2/\"\nproxy_url = \"https://{pod_id}.p.example.test:8443\"\n",
    ))
    .unwrap();
    let provider = &deployment.providers["runpod"];
    assert_eq!(
        provider.management_url, "https://runpod.example.test/v2",
        "a trailing slash is dropped"
    );
    assert_eq!(
        provider.proxy_url.as_str(),
        "https://{pod_id}.p.example.test:8443"
    );
}

/// The two keys' rules: each refusal is `config:value` naming the key and quoting nothing
/// written.
#[test]
fn runpod_host_each_key_refuses_what_its_rule_does_not_admit() {
    for (key, value) in [
        ("management_url", "http://api.runpod.io/v2"),
        ("management_url", "ftp://api.runpod.io/v2"),
        ("management_url", "https://secret-user@api.runpod.io/v2"),
        ("management_url", "https://api.runpod.io/v2?secret-query=1"),
        ("management_url", "https://api.runpod.io/v2#secret-fragment"),
        ("management_url", "https://"),
        ("proxy_url", "http://{pod_id}-{port}.proxy.runpod.net"),
        ("proxy_url", "https://{secret_placeholder}.proxy.runpod.net"),
        ("proxy_url", "https://{pod_id}.proxy.runpod.net/secret-path"),
    ] {
        let refusal = parse(&document(&format!("{key} = \"{value}\"\n"))).unwrap_err();
        assert_eq!(refusal.code(), "config:value", "{key} = {value}");
        let text = refusal.to_string();
        assert!(text.contains(&format!("providers.runpod.{key}")), "{text}");
        assert!(!text.contains("secret"), "{text}");
    }
    // A management URL beside no api_key: nothing would call it.
    let without_key = document("management_url = \"https://api.runpod.io/v2\"\n")
        .replace("api_key = \"keychain:test-gateway/runpod\"\n", "");
    let refusal = parse(&without_key).unwrap_err();
    assert_eq!(refusal.code(), "config:value");
    assert!(
        refusal
            .to_string()
            .contains("providers.runpod.management_url"),
        "{refusal}"
    );
}

/// Acceptance 7 and 9: `management_url` and `proxy_url` each pointed at the fake. The pod is
/// created through the management API with the Runpod key and reached at the proxy URL with
/// its vLLM key; a chat call without `stream` reaches the proxy with `stream: true` and usage,
/// the fake sends its first event at once and the rest over a span longer than the idle gap it
/// allows, and the client gets the one body the same events' non-streamed answer is.
#[test]
fn runpod_host_a_call_without_stream_reaches_the_proxy_streamed_and_is_answered_whole() {
    keychain();
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&both_at(&fake))).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, pods.clone()).unwrap();
    let address = relaying.local_addr();
    let wake = send(address, "POST", "/v1/models/small/wake", "");
    let answer = send(address, "POST", "/v1/chat/completions", &chat("hi", false));
    relaying.shutdown();
    assert!(wake.starts_with("HTTP/1.1 202 "), "{wake}");
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let got: Value = serde_json::from_str(body(&answer)).expect("one JSON body");
    assert_eq!(got, chat_answer());
    let seen = fake.seen();
    let management: Vec<&Seen> = seen
        .iter()
        .filter(|seen| seen.line.contains(" /v2/"))
        .collect();
    assert!(!management.is_empty());
    for call in &management {
        assert_eq!(call.host, fake.address.to_string(), "{}", call.line);
        assert!(call.head.contains(&format!(
            "authorization: bearer {}",
            RUNPOD_KEY.to_ascii_lowercase()
        )));
    }
    let created = seen
        .iter()
        .find(|seen| seen.line.starts_with("POST /v2/pods "))
        .expect("a create");
    assert_eq!(
        created.json()["gpu"],
        json!({"id": "NVIDIA L40S", "count": 1})
    );
    let relayed = seen
        .iter()
        .find(|seen| seen.line.starts_with("POST /v1/chat/completions "))
        .expect("a relayed call");
    assert_eq!(relayed.host, "pod1-8000.pods.fake.test");
    assert!(
        relayed
            .head
            .contains(&format!("authorization: bearer {VLLM_KEY}"))
    );
    let sent = relayed.json();
    assert_eq!(sent["stream"], json!(true));
    assert_eq!(sent["stream_options"], json!({"include_usage": true}));
    assert!(pods.asked().contains("pod1-8000.pods.fake.test"));
    let written = fake.written();
    assert_eq!(written.len(), CHAT_EVENTS.len());
    assert!(written[0] < GAP, "the first event at once: {written:?}");
    assert!(
        *written.last().unwrap() > IDLE,
        "a span longer than the idle gap: {written:?}"
    );
    for pair in written.windows(2) {
        assert!(
            pair[1].saturating_sub(pair[0]) < IDLE,
            "no gap past the idle bound: {written:?}"
        );
    }
}

/// Acceptance 9 on the messages wire: the body is the non-streamed message of the events.
#[test]
fn runpod_host_a_call_without_stream_is_answered_whole_on_the_messages_wire() {
    keychain();
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&both_at(&fake))).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, pods).unwrap();
    let address = relaying.local_addr();
    let _ = send(address, "POST", "/v1/models/small/wake", "");
    let answer = send(
        address,
        "POST",
        "/v1/messages",
        r#"{"model":"small","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}"#,
    );
    relaying.shutdown();
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    let got: Value = serde_json::from_str(body(&answer)).expect("one JSON body");
    assert_eq!(got, messages_answer());
    let relayed = fake
        .seen()
        .into_iter()
        .find(|seen| seen.line.starts_with("POST /v1/messages "))
        .expect("a relayed call");
    assert_eq!(relayed.json()["stream"], json!(true));
    assert!(relayed.json().get("stream_options").is_none());
}

/// Acceptance 9: a client that streams gets the events as they come; a stream the proxy cuts
/// before its end reaches the client cut, as today's upstream cut.
#[test]
fn runpod_host_a_call_without_stream_cut_by_the_proxy_reaches_the_client_cut() {
    keychain();
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&both_at(&fake))).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, pods).unwrap();
    let address = relaying.local_addr();
    let _ = send(address, "POST", "/v1/models/small/wake", "");
    let streamed = send(address, "POST", "/v1/chat/completions", &chat("hi", true));
    let cut = send(address, "POST", "/v1/chat/completions", &chat("cut", false));
    relaying.shutdown();
    assert!(streamed.starts_with("HTTP/1.1 200 "), "{streamed}");
    assert!(streamed.contains("data: [DONE]"), "{streamed}");
    assert!(cut.starts_with("HTTP/1.1 200 "), "{cut}");
    assert!(
        !cut.ends_with("0\r\n\r\n"),
        "the client sees the answer cut: {cut}"
    );
}

/// Acceptance 4 and 5: a management host whose name does not resolve. The start writes the
/// warning naming the host and the resolver's error and never the key; a wake and a call of
/// the managed model answer `target-unavailable` naming the provider; nothing is started.
#[test]
fn runpod_host_an_unresolvable_management_host_names_itself_and_starts_nothing() {
    keychain();
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(
        "management_url = \"https://runpod.invalid/v2\"\n",
    ))
    .unwrap();
    let (relaying, log) =
        logged(|| start_connected(&deployment, |transport| transport, pods.clone()).unwrap());
    let address = relaying.local_addr();
    let wake = send(address, "POST", "/v1/models/small/wake", "");
    let call = send(address, "POST", "/v1/chat/completions", &chat("hi", false));
    relaying.shutdown();
    let expected = r#"{"error":{"code":"target-unavailable","message":"no model target is available: provider runpod was not reachable at start"}}"#;
    for answer in [&wake, &call] {
        assert!(answer.starts_with("HTTP/1.1 503 "), "{answer}");
        assert_eq!(body(answer), expected);
    }
    assert!(
        fake.lines().is_empty(),
        "nothing reached Runpod: {:?}",
        fake.lines()
    );
    assert!(
        pods.asked().contains("runpod.invalid"),
        "{:?}",
        pods.asked()
    );
    let warning = log
        .lines()
        .find(|line| line.contains("Runpod REST API unreachable"))
        .unwrap_or_else(|| panic!("no startup warning in {log}"));
    assert!(warning.contains(" WARN "), "{warning}");
    assert!(
        warning.contains("provider=\"runpod\"") || warning.contains("provider=runpod"),
        "{warning}"
    );
    assert!(warning.contains("runpod.invalid"), "{warning}");
    assert!(
        warning.contains("failed to lookup address information"),
        "{warning}"
    );
    assert!(
        !log.contains(RUNPOD_KEY) && !log.contains(VLLM_KEY),
        "{log}"
    );
}

/// Acceptance 5: a management API that answers an HTTP error at start: the warning names its
/// host and the status.
#[test]
fn runpod_host_the_startup_warning_names_the_host_and_the_http_status() {
    keychain();
    let fake = Fake::start();
    fake.management(Management::Status(403));
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&both_at(&fake))).unwrap();
    let (relaying, log) =
        logged(|| start_connected(&deployment, |transport| transport, pods).unwrap());
    relaying.shutdown();
    let warning = log
        .lines()
        .find(|line| line.contains("Runpod REST API unreachable"))
        .unwrap_or_else(|| panic!("no startup warning in {log}"));
    assert!(warning.contains(&fake.address.to_string()), "{warning}");
    assert!(warning.contains("HTTP 403"), "{warning}");
    assert!(!log.contains(RUNPOD_KEY), "{log}");
}

/// Acceptance 8: once the pod runs, the management API failing (an HTTP 503, then no answer at
/// all) never breaks the inference route: each call relays through the proxy and completes.
#[test]
fn runpod_host_a_failing_management_api_never_breaks_a_running_pods_route() {
    keychain();
    let fake = Fake::start();
    let pods = Pods::new(&fake, vec![".pods.fake.test"]);
    let deployment = parse(&document(&both_at(&fake))).unwrap();
    let relaying = start_connected(&deployment, |transport| transport, pods).unwrap();
    let address = relaying.local_addr();
    let _ = send(address, "POST", "/v1/models/small/wake", "");
    let first = send(address, "POST", "/v1/chat/completions", &chat("hi", true));
    assert!(first.starts_with("HTTP/1.1 200 "), "{first}");
    let mut answers = Vec::new();
    for mode in [
        Management::Status(503),
        Management::Closed,
        Management::Status(403),
    ] {
        fake.management(mode);
        answers.push(send(
            address,
            "POST",
            "/v1/chat/completions",
            &chat("hi", false),
        ));
    }
    relaying.shutdown();
    for answer in &answers {
        assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
        let got: Value = serde_json::from_str(body(answer)).expect("one JSON body");
        assert_eq!(got, chat_answer());
    }
}

/// The assembler on each wire, fed the events split at every byte: the chat and messages
/// answers above, and on the responses wire the `response` of `response.completed`.
#[test]
fn runpod_host_the_assembler_makes_each_wires_non_streamed_answer() {
    let assembled = |wire: Wire, events: &[&str]| -> Option<Value> {
        let mut assembly = EventAssembler.begin(wire);
        for event in events {
            for byte in event.as_bytes() {
                assert!(assembly.feed(std::slice::from_ref(byte)));
            }
        }
        let bytes = assembly.finish()?;
        Some(serde_json::from_slice(&bytes).expect("JSON"))
    };
    assert_eq!(assembled(Wire::Chat, CHAT_EVENTS), Some(chat_answer()));
    assert_eq!(
        assembled(Wire::Messages, MESSAGES_EVENTS),
        Some(messages_answer())
    );
    let response = json!({"id": "resp_1", "object": "response", "status": "completed", "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Hi"}]}], "usage": {"input_tokens": 3, "output_tokens": 1}});
    let responses = [
        "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\",\"output\":[]}}\n\n".to_owned(),
        "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n".to_owned(),
        format!(
            "event: response.completed\ndata: {}\n\n",
            json!({"type": "response.completed", "response": response})
        ),
    ];
    let responses: Vec<&str> = responses.iter().map(String::as_str).collect();
    assert_eq!(assembled(Wire::Responses, &responses), Some(response));
    // A stream without its end makes no answer on any wire.
    assert_eq!(assembled(Wire::Chat, &CHAT_EVENTS[..3]), None);
    assert_eq!(assembled(Wire::Messages, &MESSAGES_EVENTS[..4]), None);
    assert_eq!(assembled(Wire::Responses, &responses[..2]), None);
}

/// The assembler's bound: a stream of `MAX_ASSEMBLED_BYTES` is read, one byte more is refused.
#[test]
fn runpod_host_the_assembler_reads_at_most_its_bound() {
    assert_eq!(MAX_ASSEMBLED_BYTES, 16 * 1024 * 1024);
    let piece = vec![b' '; 1024 * 1024];
    let mut assembly = EventAssembler.begin(Wire::Chat);
    for _ in 0..16 {
        assert!(assembly.feed(&piece));
    }
    assert!(!assembly.feed(b" "), "one byte past the bound");
}
