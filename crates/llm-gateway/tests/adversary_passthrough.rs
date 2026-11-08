//! Subscription passthrough end to end, against a loopback gateway and an in-process target
//! that stands in for the Anthropic API. The request is the shape Claude Code sends with a
//! claude.ai login and only `ANTHROPIC_BASE_URL` set (the header set the Claude Code gateway
//! protocol documents, with the OAuth token redacted to a placeholder). Nothing leaves the
//! process but the client's loopback connection; no model is called.
//!
//! What it holds (`docs/gateway.md` "Subscription passthrough"):
//! - every client line but the hop-by-hop ones, `host` and `content-length` reaches the target
//!   unchanged and in order, with the request target and its query, and the body byte for byte;
//! - a newest alias (`opus`) is resolved through `GET /v1/models?limit=1000`, sent the client's
//!   own lines, cached, and falls back to the configured model when the list fails;
//! - a route's beta extras are appended once, its static header replaces the client's line of
//!   that name, and its output cap lowers `max_tokens`;
//! - the answer reaches the client as the target sent it, but for its hop-by-hop lines;
//! - a 429 is retried after its `retry-after`, never past `x-should-retry: false`, the attempt
//!   budget or the first answer byte, and not once the client is gone; every attempt is recorded;
//! - a self-hosted route never sees the client's credential and sends its own.

use llm_gateway::{
    Gateway, GatewayConfig, IngressAuth, Label, NoOwnerVerifier, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, StaticHeader, TargetBearer, TargetRefusal, Wire,
};
use std::{
    fmt::Write as _,
    io::{self, Read, Write},
    net::TcpStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

/// Placeholder for the claude.ai OAuth access token Claude Code sends.
const TOKEN: &str = "Bearer sk-ant-oat01-REDACTED-REDACTED-REDACTED";
const BETA: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,\
                    fine-grained-tool-streaming-2025-05-14";

/// The head Claude Code sends to `ANTHROPIC_BASE_URL`, after the request line.
fn claude_code_head(session: &str) -> String {
    format!(
        "host: 127.0.0.1\r\n\
         accept: application/json\r\n\
         anthropic-beta: {BETA}\r\n\
         anthropic-dangerous-direct-browser-access: true\r\n\
         anthropic-version: 2023-06-01\r\n\
         authorization: {TOKEN}\r\n\
         content-type: application/json\r\n\
         user-agent: claude-cli/2.1.31 (external, cli)\r\n\
         x-app: cli\r\n\
         x-claude-code-session-id: {session}\r\n\
         x-stainless-arch: x64\r\n\
         x-stainless-lang: js\r\n\
         x-stainless-os: Linux\r\n\
         x-stainless-package-version: 0.70.0\r\n\
         x-stainless-retry-count: 0\r\n\
         x-stainless-runtime: node\r\n\
         x-stainless-timeout: 600\r\n"
    )
}

/// A head like Claude Code's from a client that does not retry on its own: no
/// `x-stainless-retry-count`, so the gateway retries for it.
fn retryless_head(session: &str) -> String {
    claude_code_head(session).replace("x-stainless-retry-count: 0\r\n", "")
}

fn claude_code_body(model: &str, max_tokens: u64) -> String {
    format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":[{{"type":"text","text":"say hi"}}]}}],"system":[{{"type":"text","text":"You are Claude Code."}}],"max_tokens":{max_tokens},"metadata":{{"user_id":"user_0000_account_0000_session_0000"}},"stream":true}}"#
    )
}

type Script = Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>;

/// One connection: records what the gateway writes, answers what the script says once the
/// gateway starts reading.
struct Scripted {
    written: Vec<u8>,
    log: Arc<Mutex<Vec<String>>>,
    script: Script,
    answer: Option<io::Cursor<Vec<u8>>>,
}

impl Read for Scripted {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.answer.is_none() {
            let request = String::from_utf8_lossy(&self.written).into_owned();
            self.answer = Some(io::Cursor::new((self.script)(&request)));
            self.log.lock().unwrap().push(request);
        }
        self.answer.as_mut().unwrap().read(buffer)
    }
}

impl Write for Scripted {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayStream for Scripted {}

struct Target {
    log: Arc<Mutex<Vec<String>>>,
    script: Script,
    headers: Vec<StaticHeader>,
    bearer: Option<TargetBearer>,
}

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "api.anthropic.invalid:443"
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        self.bearer.as_ref()
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.headers
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Ok(Box::new(Scripted {
            written: Vec::new(),
            log: Arc::clone(&self.log),
            script: Arc::clone(&self.script),
            answer: None,
        }))
    }
}

struct Source {
    log: Arc<Mutex<Vec<String>>>,
    script: Script,
    headers: Vec<(String, String)>,
    bearer: Option<&'static str>,
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target {
            log: Arc::clone(&self.log),
            script: Arc::clone(&self.script),
            headers: self
                .headers
                .iter()
                .map(|(name, value)| StaticHeader::new(name, value).unwrap())
                .collect(),
            bearer: self
                .bearer
                .map(|bearer| TargetBearer::new(bearer.as_bytes().to_vec()).unwrap()),
        }))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

fn answer(status: &str, lines: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{lines}content-length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

const MODEL_LIST: &str = r#"{"data":[{"type":"model","id":"claude-opus-4-7","display_name":"Claude Opus 4.7"},{"type":"model","id":"claude-sonnet-4-5-20250929","display_name":"Claude Sonnet 4.5"},{"type":"model","id":"claude-opus-4-5-20251101","display_name":"Claude Opus 4.5"}],"has_more":false}"#;

/// A target that answers the model list with `list` and every message with `message`.
fn script(list: Option<Vec<u8>>, message: Vec<u8>) -> Script {
    Arc::new(move |request: &str| {
        if request.starts_with("GET ") {
            list.clone()
                .unwrap_or_else(|| answer("500 Internal Server Error", "", "{}"))
        } else {
            message.clone()
        }
    })
}

struct Fixture {
    gateway: Option<llm_gateway::GatewayHandle>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new(model: RelayModel, source: Source) -> Self {
        let log = Arc::clone(&source.log);
        let relay = Relay::new(vec![model], Arc::new(source)).unwrap();
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.ingress_auth = IngressAuth::None;
        let gateway = Gateway::bind_with_relay(
            config,
            Arc::new(NoOwnerVerifier),
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        gateway.mark_ready();
        Self {
            gateway: Some(gateway),
            log,
        }
    }

    /// One Claude Code request carrying `head` lines; the gateway's whole answer.
    fn send(&self, head: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(self.gateway.as_ref().unwrap().local_addr()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "POST /v1/messages?beta=true HTTP/1.1\r\n{head}content-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .unwrap();
        let mut answered = Vec::new();
        drop(stream.read_to_end(&mut answered));
        String::from_utf8_lossy(&answered).into_owned()
    }

    fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(gateway) = self.gateway.take() {
            gateway.shutdown();
        }
    }
}

fn source(script: Script) -> Source {
    Source {
        log: Arc::new(Mutex::new(Vec::new())),
        script,
        headers: Vec::new(),
        bearer: None,
    }
}

fn claude(prefix: bool) -> RelayModel {
    let model = RelayModel::new(
        Label::new("claude").unwrap(),
        Label::new("claude-sonnet-4-5-20250929").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials();
    if prefix {
        model.with_model_prefix("claude-").unwrap()
    } else {
        model
    }
}

fn opus() -> RelayModel {
    RelayModel::new(
        Label::new("opus").unwrap(),
        Label::new("claude-opus-4-5-20251101").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap()
    .with_client_credentials()
    .with_newest("opus")
    .unwrap()
}

fn head_lines(request: &str) -> Vec<&str> {
    request
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .split("\r\n")
        .collect()
}

fn ok() -> Vec<u8> {
    answer("200 OK", "", r#"{"type":"message"}"#)
}

#[test]
fn adv_pt_every_end_to_end_client_header_reaches_the_target_byte_for_byte() {
    let fixture = Fixture::new(claude(true), source(script(None, ok())));
    let client = format!(
        "{}connection: keep-alive, x-hop-only\r\n\
         keep-alive: timeout=5\r\n\
         x-hop-only: dropped\r\n\
         te: trailers\r\n\
         proxy-authorization: Basic cHJveHk=\r\n\
         x-odd-value: a;b=\"c, d\"\t e\r\n",
        claude_code_head("6f1d2c3b-session")
    );
    let body = claude_code_body("claude-sonnet-4-5-20250929", 32_000);
    let answered = fixture.send(&client, &body);
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    assert_eq!(requests.len(), 1, "one call, no model list: {requests:?}");
    let (head, sent_body) = requests[0].split_once("\r\n\r\n").unwrap();
    let head: Vec<&str> = head.split("\r\n").collect();
    let hop_by_hop = [
        "host:",
        "connection:",
        "keep-alive:",
        "x-hop-only:",
        "te:",
        "proxy-authorization:",
    ];
    let mut expected = vec![
        "POST /v1/messages?beta=true HTTP/1.1".to_owned(),
        "host: api.anthropic.invalid:443".to_owned(),
    ];
    expected.extend(
        client
            .split("\r\n")
            .filter(|line| !line.is_empty())
            .filter(|line| !hop_by_hop.iter().any(|name| line.starts_with(name)))
            .map(str::to_owned),
    );
    expected.push(format!("content-length: {}", body.len()));
    expected.push("connection: close".to_owned());
    assert_eq!(head, expected, "every end-to-end line, in order, unchanged");
    assert_eq!(sent_body, body, "the body is sent byte for byte");
}

#[test]
fn adv_pt_a_newest_alias_is_resolved_with_the_clients_own_credential_and_cached() {
    let list = answer("200 OK", "", MODEL_LIST);
    let fixture = Fixture::new(opus(), source(script(Some(list), ok())));
    for _ in 0..2 {
        let answered = fixture.send(&claude_code_head("s1"), &claude_code_body("opus", 1024));
        assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    }
    let requests = fixture.requests();
    let lists: Vec<&String> = requests.iter().filter(|r| r.starts_with("GET ")).collect();
    assert_eq!(
        lists.len(),
        1,
        "the list is read once, then cached: {requests:?}"
    );
    let list_head = head_lines(lists[0]);
    assert!(
        list_head[0].starts_with("GET /v1/models?limit=1000 HTTP/1.1"),
        "{list_head:?}"
    );
    assert!(
        list_head.contains(&format!("authorization: {TOKEN}").as_str()),
        "the list is asked with the client's credential: {list_head:?}"
    );
    let calls: Vec<&String> = requests.iter().filter(|r| r.starts_with("POST ")).collect();
    assert_eq!(calls.len(), 2);
    for call in calls {
        assert!(
            call.contains(r#""model":"claude-opus-4-7""#),
            "the newest opus is sent: {call}"
        );
    }
}

#[test]
fn adv_pt_a_failed_model_list_falls_back_to_the_configured_model() {
    let fixture = Fixture::new(opus(), source(script(None, ok())));
    let answered = fixture.send(&claude_code_head("s2"), &claude_code_body("opus", 1024));
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    let call = requests.iter().find(|r| r.starts_with("POST ")).unwrap();
    assert!(
        call.contains(r#""model":"claude-opus-4-5-20251101""#),
        "{call}"
    );
}

#[test]
fn adv_pt_a_model_list_past_its_cap_falls_back_to_the_configured_model() {
    let filler = "x".repeat(1_048_577);
    let list = answer(
        "200 OK",
        "",
        &format!(r#"{{"data":[{{"id":"claude-opus-9"}}],"pad":"{filler}"}}"#),
    );
    let fixture = Fixture::new(opus(), source(script(Some(list), ok())));
    let answered = fixture.send(&claude_code_head("s3"), &claude_code_body("opus", 1024));
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    let call = requests.iter().find(|r| r.starts_with("POST ")).unwrap();
    assert!(
        call.contains(r#""model":"claude-opus-4-5-20251101""#),
        "an oversized list is not read: {}",
        &call[..call.len().min(400)]
    );
}

#[test]
fn adv_pt_route_settings_append_betas_once_override_a_static_line_and_cap_max_tokens() {
    let model = claude(true)
        .with_anthropic_beta(vec![
            "context-1m-2025-08-07".to_owned(),
            "oauth-2025-04-20".to_owned(),
        ])
        .unwrap()
        .with_max_tokens_cap(8192);
    let mut source = source(script(None, ok()));
    source.headers = vec![("anthropic-version".to_owned(), "2024-10-22".to_owned())];
    let fixture = Fixture::new(model, source);
    let answered = fixture.send(
        &claude_code_head("s4"),
        &claude_code_body("claude-opus-4-7", 32_000),
    );
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    let head = head_lines(&requests[0]);
    let betas: Vec<&&str> = head
        .iter()
        .filter(|l| l.starts_with("anthropic-beta:"))
        .collect();
    assert_eq!(
        betas,
        [&format!("anthropic-beta: {BETA},context-1m-2025-08-07").as_str()],
        "the extra is appended once and a beta the client sent is not repeated"
    );
    let versions: Vec<&&str> = head
        .iter()
        .filter(|l| l.starts_with("anthropic-version:"))
        .collect();
    assert_eq!(versions, [&"anthropic-version: 2024-10-22"], "{head:?}");
    assert!(
        requests[0].contains(r#""max_tokens":8192"#),
        "{}",
        requests[0]
    );
    let small = fixture.send(
        &claude_code_head("s4"),
        &claude_code_body("claude-opus-4-7", 512),
    );
    assert!(small.starts_with("HTTP/1.1 200 "), "{small}");
    assert!(fixture.requests()[1].contains(r#""max_tokens":512"#));
}

#[test]
fn adv_pt_the_answer_reaches_the_client_as_the_target_sent_it() {
    let long = "b".repeat(300);
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"limited"}}"#;
    let limited = answer(
        "429 Too Many Requests",
        &format!(
            "retry-after: 17\r\nx-should-retry: false\r\nrequest-id: req_011CTEST\r\n\
             anthropic-ratelimit-unified-status: rejected\r\n\
             anthropic-ratelimit-unified-reset: 1762300000\r\nx-internal-trace: kept\r\n\
             x-long: {long}\r\nconnection: keep-alive\r\nkeep-alive: timeout=5\r\n"
        ),
        body,
    );
    let fixture = Fixture::new(claude(true), source(script(None, limited)));
    let answered = fixture.send(
        &claude_code_head("s5"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    let (head, answered_body) = answered.split_once("\r\n\r\n").unwrap();
    let head: Vec<&str> = head.split("\r\n").collect();
    let long_line = format!("x-long: {long}");
    let expected = [
        "HTTP/1.1 429 Too Many Requests",
        "content-type: application/json",
        "retry-after: 17",
        "x-should-retry: false",
        "request-id: req_011CTEST",
        "anthropic-ratelimit-unified-status: rejected",
        "anthropic-ratelimit-unified-reset: 1762300000",
        "x-internal-trace: kept",
        long_line.as_str(),
        &format!("content-length: {}", body.len()),
        "connection: close",
    ];
    assert_eq!(head, expected, "every end-to-end answer line, in order");
    assert_eq!(answered_body, body);
    assert_eq!(
        fixture.requests().len(),
        1,
        "x-should-retry: false is final"
    );
}

#[test]
fn adv_pt_only_the_first_128_forwarded_client_lines_reach_the_target() {
    let fixture = Fixture::new(claude(true), source(script(None, ok())));
    let mut extra = String::new();
    for n in 0..140 {
        let _ = write!(extra, "x-extra-{n:03}: v\r\n");
    }
    let answered = fixture.send(
        &format!("host: h\r\nauthorization: {TOKEN}\r\n{extra}"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    let head = head_lines(&requests[0]);
    let forwarded = head
        .iter()
        .filter(|line| line.starts_with("authorization:") || line.starts_with("x-extra-"))
        .count();
    assert_eq!(forwarded, 128, "{head:?}");
    assert!(head.contains(&"x-extra-126: v"));
    assert!(!head.contains(&"x-extra-127: v"));
}

/// A target that answers each call with the next of `answers`, the last one from then on.
fn sequence(answers: Vec<Vec<u8>>) -> Script {
    let next = Arc::new(AtomicUsize::new(0));
    Arc::new(move |_request: &str| {
        let n = next.fetch_add(1, Ordering::SeqCst);
        answers[n.min(answers.len() - 1)].clone()
    })
}

fn limited(lines: &str, n: u32) -> Vec<u8> {
    answer(
        "429 Too Many Requests",
        lines,
        &format!(r#"{{"type":"error","error":{{"type":"rate_limit_error","message":"try {n}"}}}}"#),
    )
}

/// The attempts of the dashboard's latest call.
fn attempts(fixture: &Fixture) -> String {
    let snapshot = fixture.gateway.as_ref().unwrap().dashboard().snapshot();
    let start = snapshot.find("\"attempts\":[").expect("a call is recorded");
    let end = snapshot[start..].find("],\"session\"").unwrap();
    snapshot[start..=start + end].to_owned()
}

#[test]
fn adv_pt_a_429_with_retry_after_is_retried_and_both_attempts_are_recorded() {
    let script = sequence(vec![
        limited(
            "retry-after: 1\r\nanthropic-ratelimit-unified-status: rejected\r\n",
            1,
        ),
        ok(),
    ]);
    let fixture = Fixture::new(claude(true), source(script));
    let started = std::time::Instant::now();
    let answered = fixture.send(
        &retryless_head("r1"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    assert!(
        started.elapsed() >= Duration::from_millis(1000),
        "retry-after is honoured"
    );
    assert_eq!(fixture.requests().len(), 2);
    let recorded = attempts(&fixture);
    assert!(
        recorded.contains("\"attempt\":1,\"status\":429")
            && recorded.contains("\"attempt\":2,\"status\":200"),
        "{recorded}"
    );
    assert!(recorded.contains("\"wait_ms\":1000"), "{recorded}");
    assert!(
        recorded.contains("\"anthropic-ratelimit-unified-status\":\"rejected\""),
        "{recorded}"
    );
    assert!(
        recorded.contains("try 1"),
        "the error body is kept: {recorded}"
    );
    let metrics = fixture
        .gateway
        .as_ref()
        .unwrap()
        .dashboard()
        .render_prometheus();
    assert!(
        metrics.contains("llmgw_upstream_attempts_total{model=\"claude\",status=\"429\"} 1")
            && metrics.contains("llmgw_upstream_attempts_total{model=\"claude\",status=\"200\"} 1"),
        "{metrics}"
    );
    assert!(
        metrics.contains("llmgw_upstream_retry_wait_seconds_count{model=\"claude\"} 1"),
        "{metrics}"
    );
}

#[test]
fn adv_pt_a_client_that_retries_on_its_own_is_never_retried_by_the_gateway() {
    let script = sequence(vec![limited("x-should-retry: true\r\n", 1), ok()]);
    let fixture = Fixture::new(claude(true), source(script));
    let answered = fixture.send(
        &claude_code_head("own"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 429 "), "{answered}");
    assert!(answered.ends_with("try 1\"}}"), "{answered}");
    assert_eq!(
        fixture.requests().len(),
        1,
        "one attempt per client attempt"
    );
    let recorded = attempts(&fixture);
    assert!(
        recorded.contains("\"attempt\":1,\"status\":429"),
        "{recorded}"
    );
    assert!(!recorded.contains("\"attempt\":2"), "{recorded}");
    assert!(
        recorded.contains("try 1"),
        "the error body is kept: {recorded}"
    );

    // The same answer to a client that sends no retry count is retried.
    let script = sequence(vec![
        limited("retry-after: 0\r\nx-should-retry: true\r\n", 1),
        ok(),
    ]);
    let fixture = Fixture::new(claude(true), source(script));
    let answered = fixture.send(
        &retryless_head("none"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    assert_eq!(fixture.requests().len(), 2);
}

#[test]
fn adv_pt_x_should_retry_false_is_never_retried() {
    let script = sequence(vec![
        limited("retry-after: 0\r\nx-should-retry: false\r\n", 1),
        ok(),
    ]);
    let fixture = Fixture::new(claude(true), source(script));
    let answered = fixture.send(
        &claude_code_head("r2"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 429 "), "{answered}");
    assert_eq!(fixture.requests().len(), 1);
}

#[test]
fn adv_pt_a_spent_budget_passes_the_last_answer_through_unchanged() {
    let script = sequence(vec![
        limited("retry-after: 0\r\n", 1),
        limited("retry-after: 0\r\n", 2),
        limited("retry-after: 0\r\nx-last: yes\r\n", 3),
    ]);
    let fixture = Fixture::new(
        claude(true).with_retry(3, Duration::from_secs(60)),
        source(script),
    );
    let answered = fixture.send(
        &retryless_head("r3"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    let last = String::from_utf8(limited("retry-after: 0\r\nx-last: yes\r\n", 3)).unwrap();
    let last_body = last.split_once("\r\n\r\n").unwrap().1;
    assert!(
        answered.starts_with("HTTP/1.1 429 Too Many Requests\r\n"),
        "{answered}"
    );
    assert!(answered.contains("x-last: yes\r\n"), "{answered}");
    assert!(answered.ends_with(last_body), "{answered}");
    assert_eq!(fixture.requests().len(), 3, "three attempts, no more");

    // A retry-after past the wait budget is not waited for.
    let script = sequence(vec![limited("retry-after: 120\r\n", 1), ok()]);
    let fixture = Fixture::new(claude(true), source(script));
    let answered = fixture.send(
        &retryless_head("r4"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 429 "), "{answered}");
    assert_eq!(fixture.requests().len(), 1);
}

#[test]
fn adv_pt_nothing_is_retried_after_the_first_answer_byte() {
    // A 200 whose body is cut short: the head reached the client, so the call is over.
    let cut = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"type\"".to_vec();
    let script = sequence(vec![cut, ok()]);
    let fixture = Fixture::new(claude(true), source(script));
    let answered = fixture.send(
        &claude_code_head("r5"),
        &claude_code_body("claude-sonnet-4-5-20250929", 1024),
    );
    assert!(answered.starts_with("HTTP/1.1 200 OK\r\n"), "{answered}");
    assert_eq!(fixture.requests().len(), 1);
}

#[test]
fn adv_pt_a_client_gone_while_waiting_cancels_the_retry() {
    let script = sequence(vec![limited("retry-after: 5\r\n", 1), ok()]);
    let fixture = Fixture::new(claude(true), source(script));
    let body = claude_code_body("claude-sonnet-4-5-20250929", 1024);
    let mut stream = TcpStream::connect(fixture.gateway.as_ref().unwrap().local_addr()).unwrap();
    stream
        .write_all(
            format!(
                "POST /v1/messages HTTP/1.1\r\n{}content-length: {}\r\n\r\n{body}",
                retryless_head("r6"),
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    let dashboard = fixture.gateway.as_ref().unwrap().dashboard();
    let waiting = std::time::Instant::now();
    while !dashboard.snapshot().contains("\"waiting-retry\"") {
        assert!(waiting.elapsed() < Duration::from_secs(3), "never waited");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stream);
    let gone = std::time::Instant::now();
    while dashboard.snapshot().contains("\"in_flight\":[{") {
        assert!(
            gone.elapsed() < Duration::from_secs(2),
            "the wait was not cancelled"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fixture.requests().len(), 1, "no second attempt");
}

#[test]
fn adv_pt_a_self_hosted_route_never_sees_the_clients_credential() {
    let model = RelayModel::new(
        Label::new("qwen").unwrap(),
        Label::new("served-model").unwrap(),
        vec![Wire::Messages],
    )
    .unwrap();
    let mut source = source(script(None, ok()));
    source.bearer = Some("pod-own-key-0123456789");
    let fixture = Fixture::new(model, source);
    let answered = fixture.send(&claude_code_head("s8"), &claude_code_body("qwen", 1024));
    assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
    let requests = fixture.requests();
    assert!(
        !requests[0].contains("sk-ant-oat01") && !requests[0].contains("oauth-2025-04-20"),
        "{}",
        requests[0]
    );
    assert!(
        head_lines(&requests[0]).contains(&"authorization: Bearer pod-own-key-0123456789"),
        "{}",
        requests[0]
    );
}
