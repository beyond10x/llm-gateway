//! The gateway's counters at `GET /metrics` and its per-call usage record (story:gateway-
//! observability), one test per behaviour, each named after the row of
//! `docs/llmgw-capability-matrix.md` it closes: R4, O1 and O2. The specification is
//! `spec/domains/telemetry.yaml` and the `Relay` command of `spec/domains/gateway.yaml`.
//!
//! Every target here is in memory: its connection is a buffer holding one scripted answer, or a
//! refused connect. The gateway listens on loopback; nothing else is opened.

use llm_gateway::{
    Disposition, Gateway, GatewayConfig, GatewayHandle, Label, Metrics, OwnerToken, RefusalCode,
    Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory,
    SharedSecretVerifier, TargetRefusal, UsageRecord, UsageRecords, Wire,
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Cursor, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WAIT: Duration = Duration::from_secs(10);
const PROMETHEUS: &str = "text/plain; version=0.0.4; charset=utf-8";

/// llmgw's 13 series, in the order llm-gateway.telemetry lists them: eight process-wide, then
/// five per model and wire.
const PROCESS_SERIES: [&str; 8] = [
    "llmgw_inference_requests_total",
    "llmgw_upstream_failures_total",
    "llmgw_instruction_views_total",
    "llmgw_pod_starts_total",
    "llmgw_pod_start_failures_total",
    "llmgw_pod_reaps_total",
    "llmgw_endpoint_invalidations_total",
    "llmgw_cold_start_wait_seconds_total",
];
const ROUTE_SERIES: [&str; 5] = [
    "llmgw_route_requests_total",
    "llmgw_route_refusals_total",
    "llmgw_route_upstream_status_failures_total",
    "llmgw_route_cold_start_holds_total",
    "llmgw_route_response_bytes_total",
];

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

// --- Targets -----------------------------------------------------------------------------------

/// What the source does on one acquisition.
enum Script {
    /// Hands out a target whose connection answers these raw bytes.
    Answer(&'static str),
    /// Hands out a target that held the request this long while it started, then answers.
    Held(Duration, &'static str),
    /// Hands out a target whose connect is refused.
    Refused,
    /// Answers that the model is still starting past its hold budget.
    ColdStart,
}

#[derive(Default)]
struct Source {
    script: Mutex<VecDeque<Script>>,
    invalidated: Mutex<Vec<String>>,
}

struct Target {
    answer: Option<&'static str>,
    held: bool,
}

/// One in-memory connection: reads the scripted answer, swallows what is written.
struct Canned(Cursor<Vec<u8>>);

impl Read for Canned {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.0.read(buffer)
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
        let next = self.script.lock().unwrap().pop_front();
        match next {
            Some(Script::Answer(answer)) => Ok(Box::new(Target {
                answer: Some(answer),
                held: false,
            })),
            Some(Script::Held(wait, answer)) => {
                thread::sleep(wait);
                Ok(Box::new(Target {
                    answer: Some(answer),
                    held: true,
                }))
            }
            Some(Script::Refused) => Ok(Box::new(Target {
                answer: None,
                held: false,
            })),
            Some(Script::ColdStart) => Err(TargetRefusal::ColdStart),
            None => Err(TargetRefusal::Unavailable),
        }
    }

    fn invalidate(&self, _alias: &str, authority: &str) {
        self.invalidated.lock().unwrap().push(authority.to_owned());
    }
}

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        "pod.invalid:8000"
    }

    fn held(&self) -> bool {
        self.held
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let answer = self
            .answer
            .ok_or_else(|| io::Error::from(io::ErrorKind::ConnectionRefused))?;
        Ok(Box::new(Canned(Cursor::new(answer.as_bytes().to_vec()))))
    }
}

/// Keeps every record the gateway hands out.
#[derive(Default)]
struct Kept(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Kept {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

const ANSWER: &str =
    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 10\r\n\r\n{\"id\":\"a\"}";

// --- The gateway -------------------------------------------------------------------------------

struct Fixture {
    handle: GatewayHandle,
    source: Arc<Source>,
    kept: Arc<Kept>,
}

/// A gateway relaying `code` on chat and responses (not messages), with `script` as its source.
fn relaying(script: Vec<Script>, ready: bool) -> Fixture {
    relaying_models(
        vec![RelayModel::new(label("code"), label("served-code"), vec![Wire::Chat, Wire::Responses]).unwrap()],
        script,
        ready,
    )
}

fn relaying_models(models: Vec<RelayModel>, script: Vec<Script>, ready: bool) -> Fixture {
    let source = Arc::new(Source {
        script: Mutex::new(script.into()),
        ..Source::default()
    });
    let kept = Arc::new(Kept::default());
    let relay = Relay::new(models, Arc::clone(&source) as Arc<dyn RelayTargets>)
        .unwrap()
        .with_records(Arc::clone(&kept) as Arc<dyn UsageRecords>);
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier(),
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    if ready {
        handle.mark_ready();
    }
    Fixture {
        handle,
        source,
        kept,
    }
}

fn verifier() -> Arc<SharedSecretVerifier> {
    Arc::new(SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap())
}

impl Fixture {
    fn records(&self) -> Vec<UsageRecord> {
        self.kept.0.lock().unwrap().clone()
    }
}

// --- The client --------------------------------------------------------------------------------

struct Answered {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    elapsed: Duration,
}

impl Answered {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

fn exchange(address: SocketAddr, raw: &str) -> Answered {
    let started = Instant::now();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    let elapsed = started.elapsed();
    let text = String::from_utf8(answer).unwrap();
    let (head, rest) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(name, value)| name == "transfer-encoding" && value == "chunked");
    let body = if chunked { dechunk(rest) } else { rest.to_owned() };
    Answered {
        status,
        headers,
        body,
        elapsed,
    }
}

fn dechunk(mut rest: &str) -> String {
    let mut body = String::new();
    while let Some((size, after)) = rest.split_once("\r\n") {
        let size = usize::from_str_radix(size, 16).unwrap();
        if size == 0 {
            break;
        }
        body.push_str(&after[..size]);
        rest = &after[size + 2..];
    }
    body
}

fn credential(owner: bool) -> String {
    if owner {
        format!("authorization: Bearer {OWNER}\r\n")
    } else {
        String::new()
    }
}

fn scrape_as(address: SocketAddr, method: &str, owner: bool) -> Answered {
    exchange(
        address,
        &format!("{method} /metrics HTTP/1.1\r\nhost: g\r\n{}\r\n", credential(owner)),
    )
}

fn scrape(address: SocketAddr) -> Answered {
    let answered = scrape_as(address, "GET", true);
    assert_eq!(answered.status, 200, "{}", answered.body);
    answered
}

/// Every sample of a scrape, `name{labels}` to its value.
fn samples(answered: &Answered) -> BTreeMap<String, String> {
    answered
        .body
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let (series, value) = line.rsplit_once(' ').unwrap();
            (series.to_owned(), value.to_owned())
        })
        .collect()
}

fn route(series: &str, model: &str, wire: &str) -> String {
    format!("{series}{{model=\"{model}\",wire=\"{wire}\"}}")
}

fn post(address: SocketAddr, path: &str, body: &str, owner: bool) -> Answered {
    exchange(
        address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: g\r\n{}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            credential(owner),
            body.len()
        ),
    )
}

// --- R4: `GET /metrics` --------------------------------------------------------------------------

#[test]
fn r4_metrics_takes_the_owner_credential_and_readiness_like_inspection() {
    let unready = relaying(Vec::new(), false);
    let address = unready.handle.local_addr();
    let anonymous = scrape_as(address, "GET", false);
    assert_eq!(anonymous.status, 401, "{}", anonymous.body);
    assert!(anonymous.body.contains("\"code\":\"credential-absent\""));
    assert!(!anonymous.body.contains("llmgw_"));
    let early = scrape_as(address, "GET", true);
    assert_eq!(early.status, 503, "{}", early.body);
    assert!(early.body.contains("\"code\":\"unavailable\""));
    unready.handle.mark_ready();
    let posted = scrape_as(address, "POST", true);
    assert_eq!(posted.status, 405, "{}", posted.body);
    assert_eq!(posted.header("allow"), Some("GET, HEAD"));
    let served = scrape(address);
    assert_eq!(served.header("content-type"), Some(PROMETHEUS));
    assert_eq!(served.header("cache-control"), Some("no-store"));
    let head = scrape_as(address, "HEAD", true);
    assert_eq!(head.status, 200);
    assert_eq!(head.body, "");
    assert_eq!(
        head.header("content-length"),
        Some(served.body.len().to_string().as_str())
    );
    // A scrape is not a model call: it makes no record.
    assert!(unready.records().is_empty());
}

#[test]
fn r4_metrics_is_prometheus_text_carrying_llmgws_13_series_names_in_order() {
    let fixture = relaying(Vec::new(), true);
    let body = scrape(fixture.handle.local_addr()).body;
    let mut names = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let help = line.strip_prefix("# HELP ").unwrap_or_else(|| panic!("{line:?}"));
        let (name, text) = help.split_once(' ').unwrap();
        assert!(!text.is_empty(), "{name} has no help text");
        assert_eq!(lines.next(), Some(format!("# TYPE {name} counter").as_str()));
        while lines.peek().is_some_and(|next| !next.starts_with('#')) {
            let sample = lines.next().unwrap();
            assert!(sample.starts_with(name), "{sample:?} under {name}");
        }
        names.push(name.to_owned());
    }
    let expected: Vec<String> = PROCESS_SERIES
        .iter()
        .chain(ROUTE_SERIES.iter())
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(names, expected);
    assert!(body.ends_with('\n'));
}

#[test]
fn r4_a_gateway_without_a_relay_serves_the_process_wide_series_and_no_route() {
    let handle = Gateway::bind(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier(),
        RouteInventory::new(Vec::new()).unwrap(),
    )
    .unwrap();
    handle.mark_ready();
    let scraped = samples(&scrape(handle.local_addr()));
    let expected: BTreeMap<String, String> = PROCESS_SERIES
        .iter()
        .map(|name| ((*name).to_owned(), "0".to_owned()))
        .collect();
    assert_eq!(scraped, expected);
}

// --- O2: per-model, per-wire series ------------------------------------------------------------

#[test]
fn o2_every_declared_model_and_wire_is_registered_at_zero_before_any_request() {
    let fixture = relaying_models(
        vec![
            RelayModel::new(label("code"), label("served-code"), vec![Wire::Chat, Wire::Responses]).unwrap(),
            RelayModel::new(label("a\"b\\c"), label("served"), vec![Wire::Messages]).unwrap(),
        ],
        Vec::new(),
        true,
    );
    let scraped = samples(&scrape(fixture.handle.local_addr()));
    for series in ROUTE_SERIES {
        for (model, wire) in [("code", "chat"), ("code", "responses"), ("a\\\"b\\\\c", "messages")] {
            assert_eq!(
                scraped.get(&route(series, model, wire)).map(String::as_str),
                Some("0"),
                "{series} {model} {wire}: {scraped:?}"
            );
        }
        // A wire the model does not declare has no series.
        assert!(!scraped.contains_key(&route(series, "code", "messages")));
    }
    assert_eq!(scraped.len(), PROCESS_SERIES.len() + 3 * ROUTE_SERIES.len());
}

// --- O1, O2: one record per call, and the series it feeds ----------------------------------------

#[test]
fn o1_o2_a_relayed_answer_a_refusal_and_an_upstream_failure_each_make_one_record() {
    let fixture = relaying(vec![Script::Answer(ANSWER), Script::Refused], true);
    let address = fixture.handle.local_addr();
    let before = samples(&scrape(address));
    let relayed = post(address, "/v1/chat/completions", "{\"model\":\"code\"}", true);
    let refused = post(address, "/v1/messages", "{\"model\":\"code\"}", true);
    let failed = post(address, "/v1/responses", "{\"model\":\"code\"}", true);
    let unknown = post(address, "/v1/chat/completions", "{\"model\":\"nobody\"}", true);
    assert_eq!(relayed.status, 200);
    assert_eq!(relayed.body, "{\"id\":\"a\"}");
    assert_eq!(refused.status, 400);
    assert_eq!(failed.status, 502);
    assert_eq!(unknown.status, 404);

    let records = fixture.records();
    assert_eq!(records.len(), 4, "{records:?}");
    let expected = [
        (Some("code"), Wire::Chat, Disposition::Relayed, None, 200, 10),
        (Some("code"), Wire::Messages, Disposition::Refused, Some(RefusalCode::WireNotServed), 400, 0),
        (Some("code"), Wire::Responses, Disposition::UpstreamFailed, Some(RefusalCode::UpstreamFailed), 502, 0),
        (None, Wire::Chat, Disposition::Refused, Some(RefusalCode::ModelUnknown), 404, 0),
    ];
    let answers = [&relayed, &refused, &failed, &unknown];
    for ((record, expected), answer) in records.iter().zip(expected).zip(answers) {
        let (model, wire, disposition, refusal, status, bytes) = expected;
        assert_eq!(record.model.as_deref(), model, "{record:?}");
        assert_eq!(record.wire, wire, "{record:?}");
        assert_eq!(record.disposition, disposition, "{record:?}");
        assert_eq!(record.refusal, refusal, "{record:?}");
        assert_eq!(record.status, status, "{record:?}");
        assert_eq!(record.response_bytes, bytes, "{record:?}");
        assert!(
            u128::from(record.duration_ms) <= answer.elapsed.as_millis(),
            "{record:?} took longer than the client waited, {:?}",
            answer.elapsed
        );
        assert_eq!(record.reported_model, None);
        assert_eq!(record.input_tokens, None);
        assert_eq!(record.output_tokens, None);
        assert_eq!(record.cached_input_tokens, None);
        assert_eq!(record.cache_creation_input_tokens, None);
        assert_eq!(record.reasoning_output_tokens, None);
    }

    let after = samples(&scrape(address));
    let grew = |series: String| {
        let value = |scrape: &BTreeMap<String, String>| -> u64 {
            scrape.get(&series).unwrap_or_else(|| panic!("{series} absent")).parse().unwrap()
        };
        value(&after) - value(&before)
    };
    assert_eq!(grew("llmgw_inference_requests_total".to_owned()), 4);
    assert_eq!(grew("llmgw_upstream_failures_total".to_owned()), 1);
    assert_eq!(grew("llmgw_endpoint_invalidations_total".to_owned()), 1);
    assert_eq!(grew(route("llmgw_route_requests_total", "code", "chat")), 1);
    assert_eq!(grew(route("llmgw_route_requests_total", "code", "responses")), 1);
    assert_eq!(grew(route("llmgw_route_refusals_total", "code", "chat")), 0);
    assert_eq!(grew(route("llmgw_route_refusals_total", "code", "responses")), 0);
    assert_eq!(grew(route("llmgw_route_upstream_status_failures_total", "code", "responses")), 1);
    assert_eq!(grew(route("llmgw_route_upstream_status_failures_total", "code", "chat")), 0);
    assert_eq!(grew(route("llmgw_route_response_bytes_total", "code", "chat")), 10);
    // `code` does not declare messages: its refusal there feeds the process-wide series only.
    assert!(!after.contains_key(&route("llmgw_route_requests_total", "code", "messages")));
    assert_eq!(*fixture.source.invalidated.lock().unwrap(), vec!["pod.invalid:8000".to_owned()]);
}

#[test]
fn o1_a_relay_refused_at_its_head_after_authentication_is_recorded_and_an_anonymous_one_is_not() {
    let fixture = relaying(Vec::new(), true);
    let address = fixture.handle.local_addr();
    let anonymous = post(address, "/v1/chat/completions", "{\"model\":\"code\"}", false);
    assert_eq!(anonymous.status, 401);
    let got = exchange(
        address,
        &format!("GET /v1/chat/completions HTTP/1.1\r\nhost: g\r\n{}\r\n", credential(true)),
    );
    assert_eq!(got.status, 405);
    let none = post(address, "/v1/chat/completions", "{\"model\":\"code\"}", true);
    assert_eq!(none.status, 503);
    let records = fixture.records();
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[0].refusal, Some(RefusalCode::MethodNotAllowed));
    assert_eq!(records[0].status, 405);
    assert_eq!(records[0].model, None);
    assert_eq!(records[1].refusal, Some(RefusalCode::TargetUnavailable));
    assert_eq!(records[1].model.as_deref(), Some("code"));
    assert_eq!(records[1].disposition, Disposition::Refused);
    let scraped = samples(&scrape(address));
    assert_eq!(scraped["llmgw_inference_requests_total"], "2");
    assert_eq!(scraped[&route("llmgw_route_refusals_total", "code", "chat")], "1");
}

#[test]
fn o1_o2_a_request_held_for_a_starting_target_is_counted_with_its_wait() {
    let fixture = relaying(
        vec![
            Script::ColdStart,
            Script::Held(Duration::from_millis(120), ANSWER),
            Script::Answer(ANSWER),
        ],
        true,
    );
    let address = fixture.handle.local_addr();
    let cold = post(address, "/v1/chat/completions", "{\"model\":\"code\"}", true);
    assert_eq!(cold.status, 503);
    assert_eq!(cold.header("retry-after"), Some("30"));
    let held = post(address, "/v1/responses", "{\"model\":\"code\"}", true);
    assert_eq!(held.status, 200);
    let warm = post(address, "/v1/chat/completions", "{\"model\":\"code\"}", true);
    assert_eq!(warm.status, 200);
    let scraped = samples(&scrape(address));
    assert_eq!(scraped[&route("llmgw_route_cold_start_holds_total", "code", "chat")], "1");
    assert_eq!(scraped[&route("llmgw_route_cold_start_holds_total", "code", "responses")], "1");
    let waited: f64 = scraped["llmgw_cold_start_wait_seconds_total"].parse().unwrap();
    assert!((0.12..10.0).contains(&waited), "{waited}");
    assert_eq!(fixture.records()[0].refusal, Some(RefusalCode::ModelColdStart));
}

#[test]
fn o1_the_embeddings_pod_counters_are_served_as_counted() {
    let metrics = Arc::new(Metrics::default());
    let source = Arc::new(Source::default());
    let relay = Relay::new(
        vec![RelayModel::new(label("code"), label("served-code"), vec![Wire::Chat]).unwrap()],
        source as Arc<dyn RelayTargets>,
    )
    .unwrap()
    .with_metrics(Arc::clone(&metrics));
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier(),
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    metrics.count_pod_start();
    metrics.count_pod_start();
    metrics.count_pod_start_failure();
    metrics.count_pod_reaps(3);
    let scraped = samples(&scrape(handle.local_addr()));
    assert_eq!(scraped["llmgw_pod_starts_total"], "2");
    assert_eq!(scraped["llmgw_pod_start_failures_total"], "1");
    assert_eq!(scraped["llmgw_pod_reaps_total"], "3");
    // The metrics handed in carry the relay's routes too.
    assert_eq!(scraped[&route("llmgw_route_requests_total", "code", "chat")], "0");
    assert!(metrics.render().contains("llmgw_pod_reaps_total 3\n"));
}
