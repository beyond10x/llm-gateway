//! Adversarial pass over `story:claude-code-wire`: the `max` effort mapping on the messages
//! wire, byte-exactness of every other byte, the other wires, the body bound, and what Claude
//! Code sends beside the inference call. Driven over a real socket against loopback pods only,
//! and asserting what `docs/gateway.md` "The relay" step 6 and "Claude Code on the messages
//! wire" publish.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
    time::Duration,
};

const OWNER: &str = "owner-token-adversary-claude-code-0123456789";
const WAIT: Duration = Duration::from_secs(60);
/// `request-body-bytes`, the published bound (`docs/gateway.md`).
const BODY_BOUND: usize = 33_554_432;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

struct Pod {
    authority: String,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
    thread: Option<JoinHandle<()>>,
}

impl Pod {
    fn start(answers: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recording = Arc::clone(&seen);
        let thread = thread::spawn(move || {
            for _ in 0..answers {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                connection.set_read_timeout(Some(WAIT)).unwrap();
                connection.set_write_timeout(Some(WAIT)).unwrap();
                let Some(raw) = read_request(&mut connection) else {
                    return;
                };
                recording.lock().unwrap().push(raw);
                drop(connection.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                ));
            }
        });
        Self {
            authority,
            seen,
            thread: Some(thread),
        }
    }

    /// The bodies the pod received, in order.
    fn bodies(&self) -> Vec<Vec<u8>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|raw| raw[find(raw, b"\r\n\r\n").unwrap() + 4..].to_vec())
            .collect()
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn read_request(connection: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 1 << 16];
    let end = loop {
        if let Some(end) = find(&buffer, b"\r\n\r\n") {
            break end;
        }
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
    let wanted: usize = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    while buffer.len() < end + 4 + wanted {
        let read = connection.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Some(buffer)
}

struct Source {
    authority: String,
    acquired: Mutex<usize>,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        *self.acquired.lock().unwrap() += 1;
        Ok(Box::new(Target {
            authority: self.authority.clone(),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

struct Target {
    authority: String,
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.authority)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

struct Relayed {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
    source: Arc<Source>,
}

impl Relayed {
    fn start(pod: &Pod) -> Self {
        let source = Arc::new(Source {
            authority: pod.authority.clone(),
            acquired: Mutex::new(0),
        });
        let verifier = Arc::new(
            SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
        );
        let targets: Arc<dyn RelayTargets> = Arc::clone(&source) as Arc<dyn RelayTargets>;
        let model = RelayModel::new(
            label("code"),
            label("served-code"),
            vec![Wire::Chat, Wire::Responses, Wire::Messages],
        )
        .unwrap();
        let relay = Relay::new(vec![model], targets).unwrap();
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = WAIT;
        let handle = Gateway::bind_with_relay(
            config,
            verifier,
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
            source,
        }
    }

    fn acquired(&self) -> usize {
        *self.source.acquired.lock().unwrap()
    }
}

impl Drop for Relayed {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

/// The status line's code and the whole raw answer.
fn exchange(addr: SocketAddr, raw: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    drop(stream.write_all(raw));
    drop(stream.flush());
    let mut received = Vec::new();
    drop(stream.read_to_end(&mut received));
    let text = String::from_utf8_lossy(&received).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, text)
}

fn post(path: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    raw
}

/// Sends each body on its path and asserts the pod received each expected body, byte for byte.
fn relays(cases: &[(&str, &[u8], &[u8])]) {
    let pod = Pod::start(cases.len());
    let gateway = Relayed::start(&pod);
    for (path, sent, _) in cases {
        let (status, answer) = exchange(gateway.addr, &post(path, sent));
        assert_eq!(status, 200, "{}: {answer}", String::from_utf8_lossy(sent));
    }
    let bodies = pod.bodies();
    assert_eq!(bodies.len(), cases.len());
    for ((path, sent, expected), received) in cases.iter().zip(&bodies) {
        assert_eq!(
            String::from_utf8_lossy(received),
            String::from_utf8_lossy(expected),
            "{path} {}",
            String::from_utf8_lossy(sent)
        );
    }
}

/// vLLM (pydantic over a JSON parser that unescapes keys and values) reads an escaped key or
/// value as the plain one, so an escaped `max` reaches the template as `max` unless mapped.
#[test]
fn adv_ccw_max_written_with_escapes_in_key_or_value_is_mapped() {
    let _serial = serial();
    relays(&[
        (
            "/v1/messages",
            br#"{"model":"code","output\u005fconfig":{"eff\u006frt":"max"}}"#,
            br#"{"model":"served-code","output\u005fconfig":{"eff\u006frt":"xhigh"}}"#,
        ),
        (
            "/v1/messages",
            br#"{"model":"code","chat_template_kwargs":{"reasoning\u005feffort":"\u006d\u0061\u0078"}}"#,
            br#"{"model":"served-code","chat_template_kwargs":{"reasoning\u005feffort":"xhigh"}}"#,
        ),
    ]);
}

/// Only a string `max` at `output_config.effort` or `chat_template_kwargs.reasoning_effort` of
/// the top-level object is mapped; the same word anywhere else, or a non-string there, is not.
#[test]
fn adv_ccw_max_anywhere_but_the_two_fields_or_not_a_string_is_unchanged() {
    let _serial = serial();
    let elsewhere: &[u8] = br#"{"model":"code","output_config":{"effort":["max"],"format":{"effort":"max"},"reasoning_effort":"max"},"chat_template_kwargs":{"effort":"max","reasoning_effort":{"max":"max"}},"x":{"output_config":{"effort":"max"}},"messages":[{"output_config":{"effort":"max"},"chat_template_kwargs":{"reasoning_effort":"max"}}],"effort":"max","reasoning_effort":"max"}"#;
    let elsewhere_out =
        String::from_utf8(elsewhere.to_vec())
            .unwrap()
            .replacen("\"code\"", "\"served-code\"", 1);
    relays(&[
        ("/v1/messages", elsewhere, elsewhere_out.as_bytes()),
        (
            "/v1/messages",
            br#"{"model":"code","output_config":{"effort":null},"chat_template_kwargs":{"reasoning_effort":3}}"#,
            br#"{"model":"served-code","output_config":{"effort":null},"chat_template_kwargs":{"reasoning_effort":3}}"#,
        ),
        (
            "/v1/messages",
            br#"{"model":"code","output_config":{"effort":"max "},"chat_template_kwargs":{"reasoning_effort":"Max"}}"#,
            br#"{"model":"served-code","output_config":{"effort":"max "},"chat_template_kwargs":{"reasoning_effort":"Max"}}"#,
        ),
    ]);
}

/// Duplicate keys: every occurrence is mapped, so whichever one the pod's parser keeps is not
/// `max`.
#[test]
fn adv_ccw_every_duplicate_occurrence_of_max_is_mapped() {
    let _serial = serial();
    relays(&[
        (
            "/v1/messages",
            br#"{"output_config":{"effort":"low"},"model":"code","output_config":{"effort":"max","effort":"max"}}"#,
            br#"{"output_config":{"effort":"low"},"model":"served-code","output_config":{"effort":"xhigh","effort":"xhigh"}}"#,
        ),
        (
            "/v1/messages",
            br#"{"model":"code","chat_template_kwargs":{"reasoning_effort":"max"},"chat_template_kwargs":{"reasoning_effort":"high"}}"#,
            br#"{"model":"served-code","chat_template_kwargs":{"reasoning_effort":"xhigh"},"chat_template_kwargs":{"reasoning_effort":"xhigh"}}"#,
        ),
    ]);
}

/// Every byte but the two values is unchanged: whitespace (inside, around, leading and
/// trailing), key order, number spellings, escapes, raw UTF-8 and `cache_control`.
#[test]
fn adv_ccw_every_other_byte_of_a_pretty_body_is_unchanged() {
    let _serial = serial();
    let sent = "\r\n { \"system\" : [ {\"type\":\"text\",\"text\":\"x\",\"cache_control\":{\"type\":\"ephemeral\",\"ttl\":\"1h\"}} ],\r\n\t\"model\" :  \"code\" ,\"n\": [1E400, -0.0e-0, 0.10, 1e+2, 12345678901234567890],\n\"s\": \"\\u00e9 \u{e9} \\ud83d\\ude00 \\\" \\/ \\u0000\",\r\n \"output_config\" :\t{ \"effort\"  :  \"max\"  , \"format\": {\"type\":\"json_schema\",\"schema\":{}} } ,\"z\" : { }  }  \n\t ";
    let expected =
        sent.replacen("\"code\"", "\"served-code\"", 1)
            .replacen("\"max\"", "\"xhigh\"", 1);
    relays(&[("/v1/messages", sent.as_bytes(), expected.as_bytes())]);
}

/// On the chat and responses wires `max`, escaped or not, passes unchanged.
#[test]
fn adv_ccw_other_wires_pass_an_escaped_max_unchanged() {
    let _serial = serial();
    relays(&[
        (
            "/v1/chat/completions",
            br#"{"model":"code","output_config":{"effort":"m\u0061x"},"chat_template_kwargs":{"reasoning_effort":"max"}}"#,
            br#"{"model":"served-code","output_config":{"effort":"m\u0061x"},"chat_template_kwargs":{"reasoning_effort":"max"}}"#,
        ),
        (
            "/v1/responses",
            br#"{"model":"code","output_config":{"effort":"max"},"chat_template_kwargs":{"reasoning_effort":"m\u0061x"}}"#,
            br#"{"model":"served-code","output_config":{"effort":"max"},"chat_template_kwargs":{"reasoning_effort":"m\u0061x"}}"#,
        ),
    ]);
}

/// A messages body of exactly `request-body-bytes` carrying effort `max` is admitted, and the
/// pod receives it grown by the two rewrites, every other byte unchanged.
#[test]
fn adv_ccw_a_messages_body_at_the_bound_with_max_reaches_the_pod_mapped() {
    let _serial = serial();
    let head = br#"{"model":"code","output_config":{"effort":"max"},"pad":""#;
    let tail = br#""}"#;
    let mut sent = Vec::with_capacity(BODY_BOUND);
    sent.extend_from_slice(head);
    sent.resize(BODY_BOUND - tail.len(), b'a');
    sent.extend_from_slice(tail);
    assert_eq!(sent.len(), BODY_BOUND);
    let mut expected =
        br#"{"model":"served-code","output_config":{"effort":"xhigh"},"pad":""#.to_vec();
    expected.extend_from_slice(&sent[head.len()..]);
    let pod = Pod::start(1);
    let gateway = Relayed::start(&pod);
    let (status, answer) = exchange(gateway.addr, &post("/v1/messages", &sent));
    assert_eq!(status, 200, "{}", &answer[..answer.len().min(400)]);
    let bodies = pod.bodies();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].len(), BODY_BOUND + 7 + 2);
    assert!(
        bodies[0] == expected,
        "the pod's body differs from the mapped one"
    );
}

/// Claude Code 2.1.248 and later send the credential in both `authorization` and `x-api-key`;
/// the probe is still answered as `docs/gateway.md` says, and no target is asked.
#[test]
fn adv_ccw_head_api_hello_with_both_credential_headers_or_x_api_key_alone() {
    let _serial = serial();
    let pod = Pod::start(0);
    let gateway = Relayed::start(&pod);
    let (both, both_raw) = exchange(
        gateway.addr,
        format!(
            "HEAD /api/hello HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\nx-api-key: {OWNER}\r\nanthropic-version: 2023-06-01\r\n\r\n"
        )
        .as_bytes(),
    );
    assert_eq!(both, 404, "{both_raw}");
    assert!(both_raw.ends_with("\r\n\r\n"), "no body: {both_raw}");
    let (key_only, key_raw) = exchange(
        gateway.addr,
        format!("HEAD /api/hello HTTP/1.1\r\nhost: gateway\r\nx-api-key: {OWNER}\r\n\r\n")
            .as_bytes(),
    );
    assert_eq!(key_only, 401, "{key_raw}");
    assert!(key_raw.ends_with("\r\n\r\n"), "no body: {key_raw}");
    assert_eq!(gateway.acquired(), 0);
}

/// PROBE (green, a measurement): Claude Code's optional `POST /v1/messages/count_tokens`, which
/// the pinned vLLM v0.27.1 serves (`vllm/entrypoints/anthropic/api_router.py:95-96`), is not a
/// relayed wire path. It reaches inspection and is refused there; no target is asked.
#[test]
fn adv_ccw_probe_count_tokens_is_refused_and_asks_for_no_target() {
    let _serial = serial();
    let pod = Pod::start(0);
    let gateway = Relayed::start(&pod);
    let (status, answer) = exchange(
        gateway.addr,
        &post(
            "/v1/messages/count_tokens?beta=true",
            br#"{"model":"code","messages":[{"role":"user","content":"hi"}]}"#,
        ),
    );
    assert_eq!(status, 405, "{answer}");
    assert!(answer.contains("method-not-allowed"), "{answer}");
    assert_eq!(gateway.acquired(), 0);
}
