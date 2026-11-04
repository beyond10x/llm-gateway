//! Adversary pass on story:local-open-mode and story:live-dashboard: a gateway that
//! authenticates nobody (`IngressAuth::None`) and listens on loopback only reads no credential
//! on any path, and the dashboard it serves there keeps no header value but the user agent.
//! Every target is a loopback fixture pod; nothing leaves the machine.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel,
    RelayStream, RelayTarget, RelayTargets, RouteInventory, TargetRefusal, Wire,
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(10);
const SESSION: &str = "5b0c7e9a-local-open-session";
/// The answer of a messages stream, with the usage a self-hosted model reports.
const STREAM: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\nevent: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1200,\"cache_read_input_tokens\":64,\"output_tokens\":1}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":300}}\n\n";

/// A loopback pod answering every request with `answer`, keeping every head it read.
struct Pod {
    authority: String,
    heads: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
    stop: Arc<Mutex<bool>>,
}

impl Pod {
    fn start(answer: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&heads);
        let stop = Arc::new(Mutex::new(false));
        let stopping = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            loop {
                let Ok((mut connection, _)) = listener.accept() else {
                    return;
                };
                if *stopping.lock().unwrap() {
                    return;
                }
                connection.set_read_timeout(Some(WAIT)).unwrap();
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 8192];
                let end = loop {
                    if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        break end;
                    }
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => break 0,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                };
                let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
                let wanted: usize = head
                    .split("\r\n")
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                let mut body = buffer.len().saturating_sub(end + 4);
                while body < wanted {
                    match connection.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => body += read,
                    }
                }
                seen.lock().unwrap().push(head);
                drop(connection.write_all(answer.as_bytes()));
            }
        });
        Self {
            authority,
            heads,
            thread: Some(thread),
            stop,
        }
    }
}

impl Drop for Pod {
    fn drop(&mut self) {
        *self.stop.lock().unwrap() = true;
        drop(TcpStream::connect(&self.authority));
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

struct Source(String);

struct Target(String);

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(self.0.clone())))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn model() -> RelayModel {
    RelayModel::new(
        Label::new("local").unwrap(),
        Label::new("served-local").unwrap(),
        vec![Wire::Chat, Wire::Responses, Wire::Messages],
    )
    .unwrap()
}

/// A relaying gateway with `IngressAuth::None`, bound to `bind`, reached over loopback.
fn start(pod: &Pod, bind: &str) -> (GatewayHandle, SocketAddr) {
    let mut config = GatewayConfig::new(bind.parse().unwrap());
    config.read_timeout = WAIT;
    config.ingress_auth = IngressAuth::None;
    let inventory = RouteInventory::new(Vec::new()).unwrap();
    let relay = Relay::new(vec![model()], Arc::new(Source(pod.authority.clone()))).unwrap();
    let handle =
        Gateway::bind_with_relay(config, Arc::new(NoOwnerVerifier), inventory, relay).unwrap();
    handle.mark_ready();
    let addr = SocketAddr::from(([127, 0, 0, 1], handle.local_addr().port()));
    (handle, addr)
}

/// One request with `headers` and `body`; the whole answer, read to the close.
fn ask(addr: SocketAddr, method: &str, path: &str, headers: &[&str], body: &str) -> String {
    let mut raw = format!("{method} {path} HTTP/1.1\r\nhost: gateway\r\n");
    for header in headers {
        raw.push_str(header);
        raw.push_str("\r\n");
    }
    if !body.is_empty() {
        raw.push_str("content-type: application/json\r\ncontent-length: ");
        raw.push_str(&body.len().to_string());
        raw.push_str("\r\n");
    }
    raw.push_str("\r\n");
    raw.push_str(body);
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    drop(stream.write_all(raw.as_bytes()));
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    String::from_utf8_lossy(&answer).into_owned()
}

fn status(answer: &str) -> u16 {
    answer
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0)
}

/// Every route the gateway serves, as `(method, path, body)`: probes, public routes,
/// inspection, lifecycle, metrics, the dashboard, the three wires and the token count, and a
/// path it does not serve.
const ROUTES: [(&str, &str, &str); 18] = [
    ("GET", "/health", ""),
    ("GET", "/ready", ""),
    ("GET", "/", ""),
    ("GET", "/v1/models", ""),
    ("GET", "/v1/routes", ""),
    ("GET", "/v1/routes/local", ""),
    ("HEAD", "/v1/routes", ""),
    ("GET", "/v1/models/local/status", ""),
    ("POST", "/v1/models/local/wake", ""),
    ("GET", "/metrics", ""),
    ("GET", "/dashboard", ""),
    ("GET", "/dashboard/snapshot", ""),
    (
        "POST",
        "/v1/chat/completions",
        r#"{"model":"local","messages":[]}"#,
    ),
    ("POST", "/v1/responses", r#"{"model":"local","input":"hi"}"#),
    ("POST", "/v1/messages", r#"{"model":"local","messages":[]}"#),
    (
        "POST",
        "/v1/messages/count_tokens?beta=true",
        r#"{"model":"local","messages":[]}"#,
    ),
    (
        "POST",
        "/v1/messages?beta=true",
        r#"{"model":"local","messages":[]}"#,
    ),
    ("GET", "/not-a-route", ""),
];

/// The operator's rule: on loopback with `ingress_auth = "none"`, no path asks for a
/// credential. Sent without any `authorization`, none of them answers 401.
#[test]
fn adv_local_open_no_route_answers_401_without_a_credential() {
    let pod = Pod::start(STREAM);
    let (handle, addr) = start(&pod, "127.0.0.1:0");
    assert!(handle.is_local_open());
    for (method, path, body) in ROUTES {
        let answer = ask(addr, method, path, &[], body);
        let status = status(&answer);
        assert_ne!(status, 401, "{method} {path}: {answer}");
        assert_ne!(status, 0, "{method} {path}: no answer");
        assert!(
            !answer.contains("credential-absent"),
            "{method} {path}: {answer}"
        );
    }
    // The token count reaches the target at its own path.
    let heads = pod.heads.lock().unwrap().clone();
    assert!(
        heads
            .iter()
            .any(|head| head.starts_with("POST /v1/messages/count_tokens HTTP/1.1\r\n")),
        "{heads:?}"
    );
    // The event stream opens without a credential too; its head is all this reads.
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream
        .write_all(b"GET /dashboard/events HTTP/1.1\r\nhost: gateway\r\n\r\n")
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\n\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert!(
        head.contains("content-type: text/event-stream\r\n"),
        "{head}"
    );
    assert!(head.contains("event: snapshot\ndata: {"), "{head}");
    drop(stream);
    handle.shutdown();
}

/// Bound to every address, the same gateway is not open: only the model calls read no
/// credential, and the owner's surface still answers 401.
#[test]
fn adv_local_open_is_loopback_only() {
    let pod = Pod::start(STREAM);
    let (handle, addr) = start(&pod, "0.0.0.0:0");
    assert!(!handle.is_local_open());
    for path in [
        "/v1/routes",
        "/metrics",
        "/dashboard",
        "/dashboard/snapshot",
    ] {
        let answer = ask(addr, "GET", path, &[], "");
        assert_eq!(status(&answer), 401, "{path}: {answer}");
    }
    let answer = ask(
        addr,
        "POST",
        "/v1/messages/count_tokens",
        &[],
        r#"{"model":"local","messages":[]}"#,
    );
    assert_eq!(status(&answer), 200, "{answer}");
    handle.shutdown();
}

/// A relayed call shows on the dashboard and on `/metrics` with its tokens and a session rate,
/// keyed by a hash of the harness's session header: neither that header nor a credential the
/// client sent appears in the snapshot or the series.
#[test]
fn adv_dashboard_counts_tokens_per_session_and_keeps_no_header_value() {
    let pod = Pod::start(STREAM);
    let (handle, addr) = start(&pod, "127.0.0.1:0");
    let secret = "authorization: Bearer sk-not-a-real-key-local-open";
    let session = format!("x-claude-code-session-id: {SESSION}");
    for retry in ["0", "1"] {
        let retry = format!("x-stainless-retry-count: {retry}");
        let answer = ask(
            addr,
            "POST",
            "/v1/messages?beta=true",
            &[
                secret,
                &session,
                "user-agent: claude-cli/2.1.318 (external, cli)",
                &retry,
            ],
            r#"{"model":"local","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        );
        assert_eq!(status(&answer), 200, "{answer}");
    }
    let snapshot = ask(addr, "GET", "/dashboard/snapshot", &[], "");
    let metrics = ask(addr, "GET", "/metrics", &[], "");
    handle.shutdown();
    for text in [&snapshot, &metrics] {
        assert!(!text.contains("sk-not-a-real-key"), "{text}");
        assert!(!text.contains(SESSION), "{text}");
    }
    assert!(
        snapshot.contains("\"format\":\"llm-gateway.dashboard/1\""),
        "{snapshot}"
    );
    assert!(snapshot.contains("\"label\":\"claude-code\""), "{snapshot}");
    assert!(
        snapshot.contains(
            "\"calls\":2,\"errors\":0,\"cancelled\":0,\"input_tokens\":2400,\"output_tokens\":600"
        ),
        "{snapshot}"
    );
    assert!(snapshot.contains("\"cache_read_tokens\":64"), "{snapshot}");
    assert!(snapshot.contains("\"repeated_body\":true"), "{snapshot}");
    assert!(snapshot.contains("\"client_retry\":1"), "{snapshot}");
    assert!(snapshot.contains("\"input_tps\":"), "{snapshot}");
    assert!(
        metrics.contains("llmgw_tokens_total{model=\"local\",direction=\"input\"} 2400\n"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_tokens_total{model=\"local\",direction=\"output\"} 600\n"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_cache_tokens_total{model=\"local\",kind=\"read\"} 128\n"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_client_retries_total{model=\"local\"} 1\n"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_repeated_bodies_total{model=\"local\"} 1\n"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_session_calls_total{session=\""),
        "{metrics}"
    );
    assert!(metrics.contains("label=\"claude-code\"} 2\n"), "{metrics}");
    assert!(
        metrics.contains("llmgw_time_to_first_token_seconds_count{model=\"local\"} 2\n"),
        "{metrics}"
    );
}
