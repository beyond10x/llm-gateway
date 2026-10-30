//! The latency histogram, the outcome counter and the scrape listener
//! (`llm-gateway.telemetry`, docs/gateway.md "Scraping the counters"). The bucket bounds and
//! the refusal-code-to-outcome table are checked against the published document in
//! `src/metrics.rs`; these tests drive the gateway over loopback.
//!
//! Every target here is in memory: its connection is a buffer holding one scripted answer, a
//! refused connect, or a read that times out. The gateway listens on loopback; nothing else is
//! opened.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    collections::VecDeque,
    io::{self, Cursor, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SCRAPER: &str = "scrape-token-distinct-from-the-owner";
const WAIT: Duration = Duration::from_secs(10);
const ANSWER: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 10\r\n\r\n{\"id\":\"a\"}";
const OUTCOMES: [&str; 4] = ["ok", "upstream_error", "timeout", "refused"];

enum Script {
    Answer(&'static str),
    Refused,
    /// A target whose answer never arrives: its read times out.
    Silent,
    Unavailable,
}

#[derive(Default)]
struct Source(Mutex<VecDeque<Script>>);

struct Target(Option<Script>);

/// One in-memory connection: reads the scripted answer, or times out; swallows what is written.
struct Canned(Option<Cursor<Vec<u8>>>);

impl Read for Canned {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.0 {
            Some(answer) => answer.read(buffer),
            None => Err(io::ErrorKind::TimedOut.into()),
        }
    }
}

impl Write for Canned {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        match self.0.lock().unwrap().pop_front() {
            None | Some(Script::Unavailable) => Err(TargetRefusal::Unavailable),
            Some(script) => Ok(Box::new(Target(Some(script)))),
        }
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

impl RelayTarget for Target {
    fn authority(&self) -> &'static str {
        "pod.invalid:8000"
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        match &self.0 {
            Some(Script::Answer(answer)) => Ok(Box::new(Canned(Some(Cursor::new(
                answer.as_bytes().to_vec(),
            ))))),
            Some(Script::Silent) => Ok(Box::new(Canned(None))),
            _ => Err(io::ErrorKind::ConnectionRefused.into()),
        }
    }
}

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

/// A ready gateway relaying `code` on all three wires and `idle`, which no call names.
fn gateway(script: Vec<Script>) -> GatewayHandle {
    let models = vec![
        RelayModel::new(label("code"), label("served-code"), Wire::ALL.to_vec()).unwrap(),
        RelayModel::new(label("idle"), label("served-idle"), vec![Wire::Chat]).unwrap(),
    ];
    let relay = Relay::new(models, Arc::new(Source(Mutex::new(script.into())))).unwrap();
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

/// The status line's code and the body.
fn exchange(address: SocketAddr, raw: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    let text = String::from_utf8(answer).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, body.to_owned())
}

fn call(address: SocketAddr, path: &str, body: &str) -> u16 {
    exchange(
        address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .0
}

/// The owner's scrape of the serving listener.
fn scrape(address: SocketAddr) -> String {
    let (status, body) = exchange(
        address,
        &format!("GET /metrics HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {OWNER}\r\n\r\n"),
    );
    assert_eq!(status, 200);
    body
}

fn sample(text: &str, series: &str) -> u64 {
    text.lines()
        .find_map(|line| line.strip_prefix(series)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("no sample {series} in\n{text}"))
        .parse()
        .unwrap()
}

fn outcome(text: &str, model: &str, outcome: &str) -> u64 {
    sample(
        text,
        &format!("llmgw_requests_total{{model=\"{model}\",outcome=\"{outcome}\"}}"),
    )
}

#[test]
fn every_model_registers_its_outcomes_and_its_histogram_at_zero() {
    let gateway = gateway(Vec::new());
    let text = scrape(gateway.local_addr());
    assert!(text.contains("# TYPE llmgw_requests_total counter\n"));
    assert!(text.contains("# TYPE llmgw_request_duration_seconds histogram\n"));
    for model in ["code", "idle"] {
        for name in OUTCOMES {
            assert_eq!(outcome(&text, model, name), 0);
        }
        let inf = format!("llmgw_request_duration_seconds_bucket{{model=\"{model}\",le=\"+Inf\"}}");
        assert_eq!(sample(&text, &inf), 0);
        let count = format!("llmgw_request_duration_seconds_count{{model=\"{model}\"}}");
        assert_eq!(sample(&text, &count), 0);
    }
}

#[test]
fn the_histogram_observes_an_answer_on_each_of_the_three_wires() {
    let gateway = gateway(vec![
        Script::Answer(ANSWER),
        Script::Answer(ANSWER),
        Script::Answer(ANSWER),
    ]);
    for wire in Wire::ALL {
        assert_eq!(
            call(gateway.local_addr(), wire.path(), r#"{"model":"code"}"#),
            200
        );
    }
    let text = scrape(gateway.local_addr());
    let count = r#"llmgw_request_duration_seconds_count{model="code"}"#;
    assert_eq!(sample(&text, count), 3);
    let inf = r#"llmgw_request_duration_seconds_bucket{model="code",le="+Inf"}"#;
    assert_eq!(sample(&text, inf), 3);
    // A loopback answer from memory is well inside the 300 s bucket, and buckets only grow.
    let mut before = 0;
    for le in [
        "0.05", "0.1", "0.25", "0.5", "1", "2.5", "5", "10", "30", "60", "120", "300",
    ] {
        let bucket = format!("llmgw_request_duration_seconds_bucket{{model=\"code\",le=\"{le}\"}}");
        let value = sample(&text, &bucket);
        assert!(value >= before, "{bucket} fell to {value}");
        before = value;
    }
    assert_eq!(before, 3);
    let idle = r#"llmgw_request_duration_seconds_count{model="idle"}"#;
    assert_eq!(sample(&text, idle), 0);
}

#[test]
fn each_outcome_advances_by_one_for_exactly_its_call() {
    let gateway = gateway(vec![
        Script::Answer(ANSWER),
        Script::Refused,
        Script::Silent,
        Script::Unavailable,
    ]);
    let address = gateway.local_addr();
    let expected = [
        (200, "ok"),
        (502, "upstream_error"),
        (502, "timeout"),
        (503, "refused"),
    ];
    let mut seen = [0_u64; 4];
    for (status, name) in expected {
        assert_eq!(
            call(address, "/v1/chat/completions", r#"{"model":"code"}"#),
            status
        );
        let text = scrape(address);
        let index = OUTCOMES.iter().position(|each| *each == name).unwrap();
        seen[index] += 1;
        for (at, each) in OUTCOMES.iter().enumerate() {
            assert_eq!(
                outcome(&text, "code", each),
                seen[at],
                "after {name}: {each}"
            );
            assert_eq!(outcome(&text, "idle", each), 0);
        }
    }
}

fn scrape_listener(token: Option<&str>) -> (GatewayHandle, SocketAddr) {
    let mut gateway = gateway(Vec::new());
    let bearer = token.map(|token| OwnerToken::new(token.as_bytes().to_vec()).unwrap());
    let address = gateway
        .serve_metrics("127.0.0.1:0".parse().unwrap(), bearer)
        .unwrap();
    (gateway, address)
}

#[test]
fn the_metrics_listener_serves_the_counters_without_a_credential_and_nothing_else() {
    let (gateway, address) = scrape_listener(None);
    let (status, body) = exchange(address, "GET /metrics HTTP/1.1\r\nhost: m\r\n\r\n");
    assert_eq!(status, 200);
    assert!(body.contains("llmgw_requests_total{model=\"code\",outcome=\"ok\"} 0\n"));
    let (status, body) = exchange(address, "HEAD /metrics HTTP/1.1\r\nhost: m\r\n\r\n");
    assert_eq!((status, body.as_str()), (200, ""));
    for raw in [
        "GET / HTTP/1.1\r\nhost: m\r\n\r\n",
        "GET /health HTTP/1.1\r\nhost: m\r\n\r\n",
        "GET /ready HTTP/1.1\r\nhost: m\r\n\r\n",
        "GET /v1/models HTTP/1.1\r\nhost: m\r\n\r\n",
        "GET /v1/routes HTTP/1.1\r\nhost: m\r\n\r\n",
        "GET /metrics/ HTTP/1.1\r\nhost: m\r\n\r\n",
        "POST /metrics HTTP/1.1\r\nhost: m\r\ncontent-length: 0\r\n\r\n",
        "POST /v1/chat/completions HTTP/1.1\r\nhost: m\r\nauthorization: Bearer owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\ncontent-length: 16\r\n\r\n{\"model\":\"code\"}",
    ] {
        let (status, body) = exchange(address, raw);
        assert_eq!(status, 404, "{raw:?}");
        assert!(body.contains("path-unknown"), "{raw:?}: {body}");
    }
    // The owner-guarded `/metrics` of the serving listener is unchanged.
    let (status, body) = exchange(
        gateway.local_addr(),
        "GET /metrics HTTP/1.1\r\nhost: gateway\r\n\r\n",
    );
    assert_eq!(status, 401);
    assert!(body.contains("credential-absent"));
    let _report = gateway.shutdown();
    assert!(
        TcpStream::connect(address).is_err(),
        "the listener stops with the gateway"
    );
}

#[test]
fn a_metrics_bearer_refuses_a_missing_or_wrong_scrape_token() {
    let (_gateway, address) = scrape_listener(Some(SCRAPER));
    for (header, code) in [
        (String::new(), "credential-absent"),
        (
            "authorization: Basic abc\r\n".to_owned(),
            "credential-malformed",
        ),
        (
            format!("authorization: Bearer {OWNER}\r\n"),
            "credential-rejected",
        ),
        (
            "authorization: Bearer scrape-token\r\n".to_owned(),
            "credential-rejected",
        ),
    ] {
        let (status, body) = exchange(
            address,
            &format!("GET /metrics HTTP/1.1\r\nhost: m\r\n{header}\r\n"),
        );
        assert_eq!(status, 401, "{header:?}");
        assert!(body.contains(code), "{header:?}: {body}");
    }
    let (status, body) = exchange(
        address,
        &format!("GET /metrics HTTP/1.1\r\nhost: m\r\nauthorization: Bearer {SCRAPER}\r\n\r\n"),
    );
    assert_eq!(status, 200);
    assert!(body.contains("llmgw_request_duration_seconds_count{model=\"code\"} 0\n"));
}

#[test]
fn no_rendered_series_carries_a_key_the_owner_token_or_request_or_answer_text() {
    const PROMPT: &str = "prompt-text-zq81-never-a-label";
    const KEY: &str = "sk-key-material-zq82";
    const COMPLETION: &str = "completion-zq83";
    let answer: &'static str = Box::leak(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{{\"text\":\"{COMPLETION}\"}}",
            COMPLETION.len() + 11
        )
        .into_boxed_str(),
    );
    let body = format!(r#"{{"model":"code","messages":[{{"content":"{PROMPT}","key":"{KEY}"}}]}}"#);
    let mut gateway = gateway(vec![Script::Answer(answer), Script::Refused]);
    let address_metrics = gateway
        .serve_metrics("127.0.0.1:0".parse().unwrap(), None)
        .unwrap();
    assert_eq!(
        call(gateway.local_addr(), "/v1/chat/completions", &body),
        200
    );
    assert_eq!(call(gateway.local_addr(), "/v1/responses", &body), 502);
    for text in [
        scrape(gateway.local_addr()),
        exchange(address_metrics, "GET /metrics HTTP/1.1\r\nhost: m\r\n\r\n").1,
    ] {
        assert_eq!(outcome(&text, "code", "ok"), 1);
        for secret in [PROMPT, KEY, COMPLETION, OWNER, "served-code", "pod.invalid"] {
            assert!(!text.contains(secret), "{secret} reached a series");
        }
    }
}

/// A translator that refuses every request before a target is asked for.
struct Refuses;

impl llm_gateway::Translator for Refuses {
    fn prepare(
        &self,
        _request: &llm_gateway::TranslationRequest<'_>,
    ) -> Result<Box<dyn llm_gateway::Translation>, llm_gateway::RefusalCode> {
        Err(llm_gateway::RefusalCode::TranslationUnsupported)
    }
}

/// story:metrics-declared-wire-pairs: with a translator composed, a model's route series are
/// those of the wires it declares, and a translated request on another wire feeds the
/// process-wide series only (docs/gateway.md "Counters and usage records").
#[test]
fn a_translating_relay_registers_only_the_declared_wire_pairs() {
    let models =
        vec![RelayModel::new(label("chatonly"), label("served"), vec![Wire::Chat]).unwrap()];
    let relay = Relay::new(models, Arc::new(Source::default()))
        .unwrap()
        .with_translator(Arc::new(Refuses));
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let text = scrape(handle.local_addr());
    assert!(
        text.contains("llmgw_route_requests_total{model=\"chatonly\",wire=\"chat\"} 0\n"),
        "{text}"
    );
    assert!(
        !text.contains("model=\"chatonly\",wire=\"responses\""),
        "{text}"
    );
    assert!(
        !text.contains("model=\"chatonly\",wire=\"messages\""),
        "{text}"
    );
    let body = "{\"model\":\"chatonly\",\"input\":\"hi\"}";
    assert_eq!(call(handle.local_addr(), "/v1/responses", body), 400);
    let text = scrape(handle.local_addr());
    assert_eq!(sample(&text, "llmgw_inference_requests_total"), 1);
    assert!(
        !text.contains("model=\"chatonly\",wire=\"responses\""),
        "{text}"
    );
}
