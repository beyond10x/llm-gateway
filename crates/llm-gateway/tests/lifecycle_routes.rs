//! The owner's lifecycle routes (story:model-wake-and-status): `POST /v1/models/{alias}/wake`
//! and `GET /v1/models/{alias}/status`, answered from what the injected [`RelayTargets`] says.
//! The specification is `llm-gateway.gateway.LifecycleRoute` in `spec/domains/gateway.yaml`;
//! the scenarios are `contracts/gateway/scenarios/wake-*`.
//!
//! The target source here is a script that records every ask; no target is ever handed out,
//! so no connection leaves the gateway at all.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, Metrics, ModelKind, ModelPhase,
    ModelStatus, OwnerToken, RefusalCode, Relay, RelayModel, RelayTarget, RelayTargets,
    RouteInventory, SharedSecretVerifier, TargetRefusal, UsageRecord, UsageRecords, Wire,
};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SPEC: &str = include_str!("../../../spec/domains/gateway.yaml");
const CONTRACT: &str = include_str!("../../../docs/gateway.md");
const WAIT: Duration = Duration::from_secs(10);

/// What the source answers, per ask, and every ask it was made.
#[derive(Default)]
struct Script {
    /// `None` answers every ask `None`: a model the source starts nothing for.
    wakes: Option<VecDeque<Result<ModelStatus, TargetRefusal>>>,
    statuses: Option<VecDeque<ModelStatus>>,
    asks: Vec<String>,
}

#[derive(Default)]
struct Source(Mutex<Script>);

impl Source {
    fn scripted(
        wakes: Vec<Result<ModelStatus, TargetRefusal>>,
        statuses: Vec<ModelStatus>,
    ) -> Arc<Self> {
        Arc::new(Self(Mutex::new(Script {
            wakes: Some(wakes.into()),
            statuses: Some(statuses.into()),
            asks: Vec::new(),
        })))
    }

    fn asks(&self) -> Vec<String> {
        self.0.lock().unwrap().asks.clone()
    }
}

impl RelayTargets for Source {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.0.lock().unwrap().asks.push(format!("acquire {alias}"));
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}

    fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        let mut script = self.0.lock().unwrap();
        script.asks.push(format!("wake {alias}"));
        let wakes = script.wakes.as_mut()?;
        Some(wakes.pop_front().unwrap_or(Err(TargetRefusal::Unavailable)))
    }

    fn status(&self, alias: &str) -> Option<ModelStatus> {
        let mut script = self.0.lock().unwrap();
        script.asks.push(format!("status {alias}"));
        let statuses = script.statuses.as_mut()?;
        Some(
            statuses
                .pop_front()
                .unwrap_or(ModelStatus::new(ModelPhase::Stopped)),
        )
    }
}

fn starting(sub_phase: &str, profile: &str) -> ModelStatus {
    ModelStatus {
        phase: ModelPhase::Starting,
        sub_phase: Some(sub_phase.to_owned()),
        profile: Some(profile.to_owned()),
    }
}

fn model(alias: &str) -> RelayModel {
    RelayModel::new(
        Label::new(alias).unwrap(),
        Label::new(alias).unwrap(),
        vec![Wire::Chat],
    )
    .unwrap()
}

fn gateway(
    source: Arc<Source>,
    ready: bool,
    edit: impl FnOnce(&mut GatewayConfig),
) -> GatewayHandle {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let relay = Relay::new(vec![model("small")], source).unwrap();
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    edit(&mut config);
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    if ready {
        handle.mark_ready();
    }
    handle
}

/// One request; the whole answer, read to the close.
fn send(address: SocketAddr, method: &str, path: &str, credential: bool, body: &str) -> Answer {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let auth = if credential {
        format!("authorization: Bearer {OWNER}\r\n")
    } else {
        String::new()
    };
    let length = if body.is_empty() {
        String::new()
    } else {
        format!("content-length: {}\r\n", body.len())
    };
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nhost: gateway\r\n{auth}{length}\r\n{body}"
    )
    .unwrap();
    let mut raw = String::new();
    drop(stream.read_to_string(&mut raw));
    Answer::parse(&raw)
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    fn parse(raw: &str) -> Self {
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw, ""));
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|status| status.parse().ok())
            .unwrap_or(0);
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        Self {
            status,
            headers,
            body: body.to_owned(),
        }
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn code(&self) -> Option<String> {
        let start = self.body.find("\"code\":\"")? + 8;
        let end = self.body[start..].find('"')? + start;
        Some(self.body[start..end].to_owned())
    }
}

#[test]
fn wake_answers_202_at_once_with_the_status_the_source_reports_and_acquires_nothing() {
    let source = Source::scripted(
        vec![
            Ok(starting("pending", "l40s-32k")),
            Ok(starting("running", "l40s-32k")),
        ],
        Vec::new(),
    );
    let handle = gateway(Arc::clone(&source), true, |_| {});
    let first = send(
        handle.local_addr(),
        "POST",
        "/v1/models/small/wake",
        true,
        "",
    );
    let again = send(
        handle.local_addr(),
        "POST",
        "/v1/models/small/wake",
        true,
        "",
    );
    handle.shutdown();
    assert_eq!(first.status, 202, "{}", first.body);
    assert_eq!(
        first.body,
        "{\"model\":\"small\",\"phase\":\"starting\",\"sub_phase\":\"pending\",\"profile\":\"l40s-32k\"}"
    );
    assert_eq!(first.header("content-type"), Some("application/json"));
    assert_eq!(first.header("cache-control"), Some("no-store"));
    assert_eq!(first.header("retry-after"), None);
    assert_eq!(again.status, 202);
    assert!(
        again.body.contains("\"sub_phase\":\"running\""),
        "{}",
        again.body
    );
    assert_eq!(source.asks(), ["wake small", "wake small"]);
}

#[test]
fn status_answers_200_and_head_answers_it_without_a_body_and_neither_wakes_anything() {
    let ready = ModelStatus {
        phase: ModelPhase::Ready,
        sub_phase: None,
        profile: Some("h100-64k".to_owned()),
    };
    let source = Source::scripted(
        Vec::new(),
        vec![ready, ModelStatus::new(ModelPhase::Stopping)],
    );
    let handle = gateway(Arc::clone(&source), true, |_| {});
    let status = send(
        handle.local_addr(),
        "GET",
        "/v1/models/small/status",
        true,
        "",
    );
    let head = send(
        handle.local_addr(),
        "HEAD",
        "/v1/models/small/status",
        true,
        "",
    );
    let past = send(
        handle.local_addr(),
        "GET",
        "/v1/models/small/status",
        true,
        "",
    );
    handle.shutdown();
    assert_eq!(status.status, 200);
    assert_eq!(
        status.body,
        "{\"model\":\"small\",\"phase\":\"ready\",\"profile\":\"h100-64k\"}"
    );
    assert_eq!(head.status, 200);
    assert_eq!(head.body, "");
    assert_eq!(past.body, "{\"model\":\"small\",\"phase\":\"stopped\"}");
    assert_eq!(
        source.asks(),
        ["status small", "status small", "status small"],
        "a status asked for something other than a status"
    );
}

#[test]
fn every_phase_is_answered_by_its_code() {
    let statuses: Vec<ModelStatus> = ModelPhase::ALL.into_iter().map(ModelStatus::new).collect();
    let source = Source::scripted(Vec::new(), statuses);
    let handle = gateway(source, true, |_| {});
    let answered: Vec<String> = ModelPhase::ALL
        .iter()
        .map(|_| {
            send(
                handle.local_addr(),
                "GET",
                "/v1/models/small/status",
                true,
                "",
            )
            .body
        })
        .collect();
    handle.shutdown();
    for (phase, body) in ModelPhase::ALL.iter().zip(answered) {
        assert_eq!(
            body,
            format!("{{\"model\":\"small\",\"phase\":\"{}\"}}", phase.code())
        );
    }
}

#[test]
fn a_wake_the_source_refuses_is_answered_what_acquire_would_be() {
    let source = Source::scripted(
        TargetRefusal::ALL
            .iter()
            .map(|refusal| Err(*refusal))
            .collect(),
        Vec::new(),
    );
    let handle = gateway(source, true, |_| {});
    let answers: Vec<Answer> = TargetRefusal::ALL
        .iter()
        .map(|_| {
            send(
                handle.local_addr(),
                "POST",
                "/v1/models/small/wake",
                true,
                "",
            )
        })
        .collect();
    handle.shutdown();
    for (refusal, answer) in TargetRefusal::ALL.iter().zip(answers) {
        let code = refusal.refusal();
        assert_eq!(answer.code().as_deref(), Some(code.wire()), "{refusal:?}");
        assert_eq!(answer.status, code.status());
        let retry_after = (code == RefusalCode::ModelColdStart).then_some("30");
        assert_eq!(answer.header("retry-after"), retry_after, "{refusal:?}");
    }
}

#[test]
fn a_model_the_source_starts_nothing_for_is_path_unknown_and_an_unknown_alias_model_unknown() {
    let source = Arc::new(Source::default());
    let handle = gateway(Arc::clone(&source), true, |_| {});
    let address = handle.local_addr();
    let wake = send(address, "POST", "/v1/models/small/wake", true, "");
    let status = send(address, "GET", "/v1/models/small/status", true, "");
    let unknown_wake = send(address, "POST", "/v1/models/absent/wake", true, "");
    let unknown_status = send(address, "GET", "/v1/models/absent/status", true, "");
    handle.shutdown();
    assert_eq!(
        wake.code().as_deref(),
        Some("path-unknown"),
        "{}",
        wake.body
    );
    assert_eq!(status.code().as_deref(), Some("path-unknown"));
    assert_eq!(unknown_wake.code().as_deref(), Some("model-unknown"));
    assert_eq!(unknown_status.code().as_deref(), Some("model-unknown"));
    assert_eq!(
        source.asks(),
        ["wake small", "status small"],
        "an unknown alias reached the source"
    );
}

#[test]
fn the_owner_credential_the_method_a_body_and_readiness_are_decided_before_the_source_is_asked() {
    let source = Source::scripted(vec![Ok(starting("running", "default"))], Vec::new());
    let unready = gateway(Arc::clone(&source), false, |_| {});
    let early = send(
        unready.local_addr(),
        "POST",
        "/v1/models/small/wake",
        true,
        "",
    );
    unready.shutdown();
    // Bound to every address: on loopback alone, `IngressAuth::None` opens every path
    // (story:local-open-mode).
    let handle = gateway(Arc::clone(&source), true, |config| {
        config.ingress_auth = IngressAuth::None;
        config.bind = "0.0.0.0:0".parse().unwrap();
    });
    let address = SocketAddr::from(([127, 0, 0, 1], handle.local_addr().port()));
    let anonymous_wake = send(address, "POST", "/v1/models/small/wake", false, "");
    let anonymous_status = send(address, "GET", "/v1/models/small/status", false, "");
    let get_wake = send(address, "GET", "/v1/models/small/wake", true, "");
    let post_status = send(address, "POST", "/v1/models/small/status", true, "");
    let with_body = send(address, "POST", "/v1/models/small/wake", true, "{}");
    let empty_alias = send(address, "POST", "/v1/models//wake", true, "");
    let served = send(address, "POST", "/v1/models/small/wake", true, "");
    handle.shutdown();
    assert_eq!(early.code().as_deref(), Some("unavailable"));
    for anonymous in [&anonymous_wake, &anonymous_status] {
        assert_eq!(anonymous.code().as_deref(), Some("credential-absent"));
        assert_eq!(anonymous.header("www-authenticate"), Some("Bearer"));
    }
    assert_eq!(get_wake.code().as_deref(), Some("method-not-allowed"));
    assert_eq!(get_wake.header("allow"), Some("POST"));
    assert_eq!(post_status.code().as_deref(), Some("method-not-allowed"));
    assert_eq!(post_status.header("allow"), Some("GET, HEAD"));
    assert_eq!(with_body.code().as_deref(), Some("body-not-allowed"));
    assert_eq!(
        empty_alias.code().as_deref(),
        Some("method-not-allowed"),
        "an empty alias is no lifecycle path: {}",
        empty_alias.body
    );
    assert_eq!(served.status, 202, "{}", served.body);
    assert_eq!(source.asks(), ["wake small"]);
}

#[test]
fn without_a_relay_the_lifecycle_paths_are_paths_the_gateway_does_not_serve() {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
    )
    .unwrap();
    handle.mark_ready();
    let status = send(
        handle.local_addr(),
        "GET",
        "/v1/models/small/status",
        true,
        "",
    );
    let wake = send(
        handle.local_addr(),
        "POST",
        "/v1/models/small/wake",
        true,
        "",
    );
    handle.shutdown();
    assert_eq!(status.code().as_deref(), Some("path-unknown"));
    assert_eq!(wake.code().as_deref(), Some("method-not-allowed"));
    assert_eq!(wake.header("allow"), Some("GET, HEAD"));
}

/// The phase codes the crate answers are exactly `llm-gateway.gateway.ModelPhase`'s, in order,
/// and the published route table names both routes with the methods they take.
#[test]
fn every_phase_and_both_routes_are_declared_and_published() {
    let start = SPEC
        .find("  - name: llm-gateway.gateway.ModelPhase\n")
        .expect("spec/domains/gateway.yaml declares no ModelPhase");
    let rest = &SPEC[start + 1..];
    let declaration = &rest[..rest.find("\n  - name: ").unwrap_or(rest.len())];
    let declared: Vec<&str> = declaration
        .lines()
        .filter_map(|line| line.split_once("code: ").map(|(_, code)| code))
        .map(|code| code.trim_end_matches("}}"))
        .collect();
    let crate_codes: Vec<&str> = ModelPhase::ALL.iter().map(|phase| phase.code()).collect();
    assert_eq!(declared, crate_codes);
    for row in [
        "| `POST` | `/v1/models/{alias}/wake` | owner |",
        "| `GET`, `HEAD` | `/v1/models/{alias}/status` | owner |",
        "| `GET`, `HEAD` | `/v1/status` | owner |",
    ] {
        assert!(CONTRACT.contains(row), "docs/gateway.md has no row {row}");
    }
}

// --- The status list (story:route-status-for-every-model) ----------------------------------

/// A source that answers each model's status from a table, after `delay`, and records every ask
/// of every kind.
struct Table {
    statuses: BTreeMap<String, Option<ModelStatus>>,
    delay: Duration,
    asks: Mutex<Vec<String>>,
}

impl Table {
    fn new(statuses: &[(&str, Option<ModelStatus>)], delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            statuses: statuses
                .iter()
                .map(|(alias, status)| ((*alias).to_owned(), status.clone()))
                .collect(),
            delay,
            asks: Mutex::new(Vec::new()),
        })
    }

    fn asks(&self) -> Vec<String> {
        let mut asks = self.asks.lock().unwrap().clone();
        asks.sort();
        asks
    }
}

impl RelayTargets for Table {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.asks.lock().unwrap().push(format!("acquire {alias}"));
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, alias: &str, _authority: &str) {
        self.asks
            .lock()
            .unwrap()
            .push(format!("invalidate {alias}"));
    }

    fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        self.asks.lock().unwrap().push(format!("wake {alias}"));
        None
    }

    fn status(&self, alias: &str) -> Option<ModelStatus> {
        self.asks.lock().unwrap().push(format!("status {alias}"));
        thread::sleep(self.delay);
        self.statuses.get(alias).cloned().flatten()
    }
}

/// Every usage record the relay hands out.
#[derive(Default)]
struct Records(Mutex<Vec<String>>);

impl UsageRecords for Records {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(format!("{record:?}"));
    }
}

fn kinded(alias: &str, kind: ModelKind, wires: Vec<Wire>) -> RelayModel {
    RelayModel::new(
        Label::new(alias).unwrap(),
        Label::new(alias).unwrap(),
        wires,
    )
    .unwrap()
    .with_kind(kind)
}

fn status(phase: ModelPhase, sub_phase: Option<&str>, profile: Option<&str>) -> ModelStatus {
    ModelStatus {
        phase,
        sub_phase: sub_phase.map(str::to_owned),
        profile: profile.map(str::to_owned),
    }
}

/// A gateway relaying `models` to `source`, ready, counting into `metrics` and recording into
/// `records`.
fn listing(
    models: Vec<RelayModel>,
    source: Arc<Table>,
    metrics: &Arc<Metrics>,
    records: &Arc<Records>,
    edit: impl FnOnce(&mut GatewayConfig),
) -> GatewayHandle {
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let relay = Relay::new(models, source)
        .unwrap()
        .with_metrics(Arc::clone(metrics))
        .with_records(Arc::clone(records) as Arc<dyn UsageRecords>);
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    edit(&mut config);
    Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap()
}

/// Every kind, every phase and a model without a status, in one list: alias order, the keys in
/// their published order, `callable` true for `ready`, `starting` and `stopped` and false for
/// `stopping` and `unreachable`, and neither `phase` nor `callable` without a status.
#[test]
fn route_status_the_list_names_every_model_in_alias_order_with_kind_wires_status_and_callable() {
    let source = Table::new(
        &[
            ("a-stopped", Some(status(ModelPhase::Stopped, None, None))),
            (
                "b-starting",
                Some(status(
                    ModelPhase::Starting,
                    Some("pending"),
                    Some("l40s-32k"),
                )),
            ),
            (
                "c-ready",
                Some(status(ModelPhase::Ready, None, Some("h100-64k"))),
            ),
            ("d-stopping", Some(status(ModelPhase::Stopping, None, None))),
            ("e-hosted", Some(status(ModelPhase::Ready, None, None))),
            (
                "f-route",
                Some(status(
                    ModelPhase::Unreachable,
                    Some("credential-refused"),
                    None,
                )),
            ),
            ("g-bedrock", None),
        ],
        Duration::ZERO,
    );
    // Declared out of alias order, so the list's order is the gateway's own.
    let models = vec![
        kinded("g-bedrock", ModelKind::Bedrock, vec![Wire::Messages]),
        kinded("f-route", ModelKind::Route, vec![Wire::Chat]),
        kinded(
            "e-hosted",
            ModelKind::Hosted,
            vec![Wire::Responses, Wire::Chat],
        ),
        model("d-stopping"),
        model("c-ready"),
        model("b-starting"),
        RelayModel::new(
            Label::new("a-stopped").unwrap(),
            Label::new("a-stopped").unwrap(),
            vec![Wire::Chat, Wire::Responses],
        )
        .unwrap(),
    ];
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let list = send(handle.local_addr(), "GET", "/v1/status", true, "");
    handle.shutdown();
    assert_eq!(list.status, 200, "{}", list.body);
    assert_eq!(list.header("content-type"), Some("application/json"));
    assert_eq!(list.header("cache-control"), Some("no-store"));
    assert_eq!(
        list.body,
        concat!(
            "{\"format\":\"llm-gateway.status/1\",\"models\":[",
            "{\"model\":\"a-stopped\",\"kind\":\"pod\",\"wires\":[\"chat\",\"responses\"],\"phase\":\"stopped\",\"callable\":true},",
            "{\"model\":\"b-starting\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"starting\",\"sub_phase\":\"pending\",\"profile\":\"l40s-32k\",\"callable\":true},",
            "{\"model\":\"c-ready\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"ready\",\"profile\":\"h100-64k\",\"callable\":true},",
            "{\"model\":\"d-stopping\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"stopping\",\"callable\":false},",
            "{\"model\":\"e-hosted\",\"kind\":\"hosted\",\"wires\":[\"responses\",\"chat\"],\"phase\":\"ready\",\"callable\":true},",
            "{\"model\":\"f-route\",\"kind\":\"route\",\"wires\":[\"chat\"],\"phase\":\"unreachable\",\"sub_phase\":\"credential-refused\",\"callable\":false},",
            "{\"model\":\"g-bedrock\",\"kind\":\"bedrock\",\"wires\":[\"messages\"]}",
            "]}"
        )
    );
}

/// What `callable` says for each phase, and that a model's kind is `pod` unless declared.
#[test]
fn route_status_callable_is_true_exactly_where_a_call_is_relayed_or_held() {
    let callable: Vec<(&str, bool)> = ModelPhase::ALL
        .iter()
        .map(|phase| (phase.code(), phase.callable()))
        .collect();
    assert_eq!(
        callable,
        [
            ("stopped", true),
            ("starting", true),
            ("ready", true),
            ("stopping", false),
            ("unreachable", false),
        ]
    );
    assert_eq!(model("small").kind(), ModelKind::Pod);
    let codes: Vec<&str> = ModelKind::ALL.iter().map(|kind| kind.code()).collect();
    assert_eq!(codes, ["pod", "hosted", "route", "bedrock", "ollama"]);
}

/// story:a-call-starts-no-pod: a model declared with `RelayModel::with_no_call_start` is listed
/// `callable: false` while `stopped`, and as any other model in every other phase; an undeclared
/// model keeps `stopped` callable.
#[test]
fn callable_a_model_whose_call_starts_nothing_is_not_callable_while_stopped() {
    let phases: Vec<(String, Option<ModelStatus>)> = ModelPhase::ALL
        .iter()
        .map(|phase| {
            (
                format!("n-{}", phase.code()),
                Some(status(*phase, None, None)),
            )
        })
        .chain([(
            "u-stopped".to_owned(),
            Some(status(ModelPhase::Stopped, None, None)),
        )])
        .collect();
    let rows: Vec<(&str, Option<ModelStatus>)> = phases
        .iter()
        .map(|(alias, status)| (alias.as_str(), status.clone()))
        .collect();
    let source = Table::new(&rows, Duration::ZERO);
    let mut models: Vec<RelayModel> = ModelPhase::ALL
        .iter()
        .map(|phase| model(&format!("n-{}", phase.code())).with_no_call_start())
        .collect();
    models.push(model("u-stopped"));
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let list = send(handle.local_addr(), "GET", "/v1/status", true, "");
    handle.shutdown();
    assert_eq!(list.status, 200, "{}", list.body);
    assert_eq!(
        list.body,
        concat!(
            "{\"format\":\"llm-gateway.status/1\",\"models\":[",
            "{\"model\":\"n-ready\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"ready\",\"callable\":true},",
            "{\"model\":\"n-starting\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"starting\",\"callable\":true},",
            "{\"model\":\"n-stopped\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"stopped\",\"callable\":false},",
            "{\"model\":\"n-stopping\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"stopping\",\"callable\":false},",
            "{\"model\":\"n-unreachable\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"unreachable\",\"callable\":false},",
            "{\"model\":\"u-stopped\",\"kind\":\"pod\",\"wires\":[\"chat\"],\"phase\":\"stopped\",\"callable\":true}",
            "]}"
        )
    );
}

/// The statuses are asked side by side: three models whose status takes 800 ms each are listed
/// in well under twice that, not in 2.4 s.
#[test]
fn route_status_the_list_asks_every_status_side_by_side() {
    let slow = Duration::from_millis(800);
    let source = Table::new(
        &[
            ("one", Some(status(ModelPhase::Ready, None, None))),
            ("two", Some(status(ModelPhase::Ready, None, None))),
            ("three", Some(status(ModelPhase::Ready, None, None))),
        ],
        slow,
    );
    let models = ["one", "two", "three"]
        .into_iter()
        .map(|alias| kinded(alias, ModelKind::Hosted, vec![Wire::Chat]))
        .collect();
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let started = Instant::now();
    let list = send(handle.local_addr(), "GET", "/v1/status", true, "");
    let took = started.elapsed();
    handle.shutdown();
    assert_eq!(list.status, 200, "{}", list.body);
    assert!(took >= slow, "{took:?}");
    assert!(
        took < slow * 2,
        "the statuses were asked one after another: {took:?}"
    );
    assert_eq!(source.asks(), ["status one", "status three", "status two"]);
}

/// A status read, one model's or the list, asks the source for statuses and nothing else, makes
/// no usage record and moves no counter.
#[test]
fn route_status_a_status_read_acquires_wakes_and_invalidates_nothing_and_records_nothing() {
    let source = Table::new(
        &[
            ("hosted", Some(status(ModelPhase::Ready, None, None))),
            ("pod", Some(status(ModelPhase::Stopped, None, None))),
        ],
        Duration::ZERO,
    );
    let models = vec![
        kinded("hosted", ModelKind::Hosted, vec![Wire::Chat]),
        model("pod"),
    ];
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let before = metrics.render();
    let address = handle.local_addr();
    let list = send(address, "GET", "/v1/status", true, "");
    let head = send(address, "HEAD", "/v1/status", true, "");
    let one = send(address, "GET", "/v1/models/hosted/status", true, "");
    let after = metrics.render();
    handle.shutdown();
    assert_eq!(list.status, 200, "{}", list.body);
    assert_eq!(head.status, 200);
    assert_eq!(head.body, "");
    assert_eq!(one.body, "{\"model\":\"hosted\",\"phase\":\"ready\"}");
    assert_eq!(
        source.asks(),
        [
            "status hosted",
            "status hosted",
            "status hosted",
            "status pod",
            "status pod"
        ]
    );
    assert!(
        records.0.lock().unwrap().is_empty(),
        "a status read made a record"
    );
    assert_eq!(before, after, "a status read moved a counter");
}

/// `GET /v1/launch` asks the status of a pod model only: a hosted model's, a route's and a
/// Bedrock model's status would be a probe of an endpoint (or nothing), and the launch route asks
/// no endpoint. Their `phase` is absent, as before the status list existed.
#[test]
fn route_status_the_launch_route_asks_the_status_of_a_pod_model_only() {
    let source = Table::new(
        &[
            ("a-pod", Some(status(ModelPhase::Stopped, None, None))),
            ("b-hosted", Some(status(ModelPhase::Ready, None, None))),
            ("c-route", Some(status(ModelPhase::Ready, None, None))),
            ("d-bedrock", None),
        ],
        Duration::ZERO,
    );
    let models = vec![
        model("a-pod"),
        kinded("b-hosted", ModelKind::Hosted, vec![Wire::Chat]),
        kinded("c-route", ModelKind::Route, vec![Wire::Chat]),
        kinded("d-bedrock", ModelKind::Bedrock, vec![Wire::Messages]),
    ];
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let launch = send(handle.local_addr(), "GET", "/v1/launch", true, "");
    handle.shutdown();
    assert_eq!(launch.status, 200, "{}", launch.body);
    assert_eq!(source.asks(), ["status a-pod"]);
    assert_eq!(
        launch.body.matches("\"phase\":null").count(),
        3,
        "{}",
        launch.body
    );
    assert!(
        launch.body.contains("\"phase\":\"stopped\""),
        "{}",
        launch.body
    );
}

/// The list takes the owner credential under `IngressAuth::None` too, and its method, a body
/// and readiness are decided before the source is asked anything. A gateway without a relay
/// lists no model.
#[test]
fn route_status_the_list_takes_the_owner_and_is_refused_before_the_source_is_asked() {
    let source = Table::new(&[("small", None)], Duration::ZERO);
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(
        vec![model("small")],
        Arc::clone(&source),
        &metrics,
        &records,
        |config| {
            config.ingress_auth = IngressAuth::None;
            config.bind = "0.0.0.0:0".parse().unwrap();
        },
    );
    let address = SocketAddr::from(([127, 0, 0, 1], handle.local_addr().port()));
    let unready = send(address, "GET", "/v1/status", true, "");
    handle.mark_ready();
    let anonymous = send(address, "GET", "/v1/status", false, "");
    let post = send(address, "POST", "/v1/status", true, "");
    let with_body = send(address, "GET", "/v1/status", true, "{}");
    assert!(source.asks().is_empty(), "{:?}", source.asks());
    let served = send(address, "GET", "/v1/status", true, "");
    handle.shutdown();
    assert_eq!(unready.code().as_deref(), Some("unavailable"));
    assert_eq!(anonymous.code().as_deref(), Some("credential-absent"));
    assert_eq!(anonymous.header("www-authenticate"), Some("Bearer"));
    assert_eq!(post.code().as_deref(), Some("method-not-allowed"));
    assert_eq!(post.header("allow"), Some("GET, HEAD"));
    assert_eq!(with_body.code().as_deref(), Some("body-not-allowed"));
    assert_eq!(
        served.body,
        "{\"format\":\"llm-gateway.status/1\",\"models\":[{\"model\":\"small\",\"kind\":\"pod\",\"wires\":[\"chat\"]}]}"
    );
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let bare = Gateway::bind(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
    )
    .unwrap();
    bare.mark_ready();
    let empty = send(bare.local_addr(), "GET", "/v1/status", true, "");
    bare.shutdown();
    assert_eq!(
        empty.body,
        "{\"format\":\"llm-gateway.status/1\",\"models\":[]}"
    );
}

/// `ModelKind`'s codes are `llm-gateway.gateway.ModelKind`'s, in order, and the list's format
/// and path are `llm-gateway.gateway.StatusRoute`'s.
#[test]
fn route_status_every_kind_and_the_list_are_declared() {
    let start = SPEC
        .find("  - name: llm-gateway.gateway.ModelKind\n")
        .expect("spec/domains/gateway.yaml declares no ModelKind");
    let rest = &SPEC[start + 1..];
    let declaration = &rest[..rest.find("\n  - name: ").unwrap_or(rest.len())];
    let declared: Vec<&str> = declaration
        .lines()
        .filter_map(|line| line.split_once("code: ").map(|(_, code)| code))
        .map(|code| code.trim_end_matches("}}"))
        .collect();
    let codes: Vec<&str> = ModelKind::ALL.iter().map(|kind| kind.code()).collect();
    assert_eq!(declared, codes);
    assert!(
        SPEC.contains(
            "{name: List, attributes: {path: /v1/status, methods: \"GET, HEAD\", format: llm-gateway.status/1}}"
        ),
        "spec/domains/gateway.yaml declares no StatusRoute List"
    );
}

// --- story:ollama-route ------------------------------------------------------------------------

/// Acceptance 3 in the list: an Ollama model is listed with kind `ollama`, and `callable` is
/// its phase's alone: `ready` and `stopped` `not-loaded` (a call loads the model) are callable,
/// `unreachable` for either reason is not.
#[test]
fn ollama_the_list_names_an_ollama_model_by_its_kind_and_callable_by_its_phase() {
    let all = vec![Wire::Chat, Wire::Responses, Wire::Messages];
    let source = Table::new(
        &[
            ("a-ready", Some(status(ModelPhase::Ready, None, None))),
            (
                "b-not-loaded",
                Some(status(ModelPhase::Stopped, Some("not-loaded"), None)),
            ),
            (
                "c-not-pulled",
                Some(status(
                    ModelPhase::Unreachable,
                    Some("model-not-pulled"),
                    None,
                )),
            ),
            (
                "d-silent",
                Some(status(ModelPhase::Unreachable, Some("no-answer"), None)),
            ),
        ],
        Duration::ZERO,
    );
    let models = ["a-ready", "b-not-loaded", "c-not-pulled", "d-silent"]
        .into_iter()
        .map(|alias| kinded(alias, ModelKind::Ollama, all.clone()))
        .collect();
    let (metrics, records) = (Arc::new(Metrics::default()), Arc::new(Records::default()));
    let handle = listing(models, Arc::clone(&source), &metrics, &records, |_| {});
    handle.mark_ready();
    let list = send(handle.local_addr(), "GET", "/v1/status", true, "");
    handle.shutdown();
    assert_eq!(list.status, 200, "{}", list.body);
    let wires = "\"wires\":[\"chat\",\"responses\",\"messages\"]";
    assert_eq!(
        list.body,
        format!(
            "{{\"format\":\"llm-gateway.status/1\",\"models\":[\
             {{\"model\":\"a-ready\",\"kind\":\"ollama\",{wires},\"phase\":\"ready\",\"callable\":true}},\
             {{\"model\":\"b-not-loaded\",\"kind\":\"ollama\",{wires},\"phase\":\"stopped\",\"sub_phase\":\"not-loaded\",\"callable\":true}},\
             {{\"model\":\"c-not-pulled\",\"kind\":\"ollama\",{wires},\"phase\":\"unreachable\",\"sub_phase\":\"model-not-pulled\",\"callable\":false}},\
             {{\"model\":\"d-silent\",\"kind\":\"ollama\",{wires},\"phase\":\"unreachable\",\"sub_phase\":\"no-answer\",\"callable\":false}}\
             ]}}"
        )
    );
    assert!(records.0.lock().unwrap().is_empty());
}

/// A source that reports every model not pulled on its Ollama server, recording each ask.
#[derive(Default)]
struct Unpulled(Mutex<Vec<String>>);

impl RelayTargets for Unpulled {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        self.0.lock().unwrap().push(format!("acquire {alias}"));
        Err(TargetRefusal::NotPulled)
    }

    fn invalidate(&self, alias: &str, _authority: &str) {
        self.0.lock().unwrap().push(format!("invalidate {alias}"));
    }
}

/// Every usage record the relay hands out, whole.
#[derive(Default)]
struct Kept(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Kept {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

/// Acceptance 4: a call to a model its Ollama server has not pulled is refused `503`
/// `model-not-pulled` on every wire, with no `retry-after`, its message the fixed text followed
/// by `: ` and the alias of the model the call named (the declaring alias for a call matched by
/// a `model_prefix`, never the caller's own model name), and its usage record is a refusal with
/// no target status and the outcome `refused`.
#[test]
fn ollama_a_model_not_pulled_is_refused_503_naming_its_alias_without_retry_after() {
    let all = vec![Wire::Chat, Wire::Responses, Wire::Messages];
    let models = vec![
        kinded("qwen-local", ModelKind::Ollama, all.clone()),
        kinded("qwen-family", ModelKind::Ollama, all)
            .with_model_prefix("qwen2.5:")
            .unwrap(),
    ];
    let source = Arc::new(Unpulled::default());
    let records = Arc::new(Kept::default());
    let verifier = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let relay = Relay::new(models, Arc::clone(&source) as Arc<dyn RelayTargets>)
        .unwrap()
        .with_records(Arc::clone(&records) as Arc<dyn UsageRecords>);
    let handle = Gateway::bind_with_relay(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    let calls = [
        ("/v1/chat/completions", "qwen-local", "qwen-local"),
        ("/v1/responses", "qwen-local", "qwen-local"),
        ("/v1/messages", "qwen-local", "qwen-local"),
        ("/v1/chat/completions", "qwen2.5:7b", "qwen-family"),
    ];
    let reason = RefusalCode::ModelNotPulled.reason();
    for (path, named, alias) in calls {
        let body = format!("{{\"model\":\"{named}\",\"max_tokens\":8}}");
        let answer = send(handle.local_addr(), "POST", path, true, &body);
        assert_eq!(answer.status, 503, "{path} {}", answer.body);
        assert_eq!(answer.header("retry-after"), None, "{path}");
        assert_eq!(
            answer.body,
            format!(
                "{{\"error\":{{\"code\":\"model-not-pulled\",\"message\":\"{reason}: {alias}\"}}}}"
            ),
            "{path}"
        );
    }
    handle.shutdown();
    assert_eq!(
        *source.0.lock().unwrap(),
        [
            "acquire qwen-local",
            "acquire qwen-local",
            "acquire qwen-local",
            "acquire qwen-family"
        ]
    );
    let records = records.0.lock().unwrap();
    let seen: Vec<_> = records
        .iter()
        .map(|record| {
            (
                record.model.as_deref(),
                record.wire,
                record.disposition,
                record.refusal,
                record.outcome,
                record.status,
                record.target_status,
                record.response_bytes,
            )
        })
        .collect();
    let refused = |alias, wire| {
        (
            Some(alias),
            wire,
            llm_gateway::Disposition::Refused,
            Some(RefusalCode::ModelNotPulled),
            llm_gateway::Outcome::Refused,
            503,
            None,
            0,
        )
    };
    assert_eq!(
        seen,
        [
            refused("qwen-local", Wire::Chat),
            refused("qwen-local", Wire::Responses),
            refused("qwen-local", Wire::Messages),
            refused("qwen-family", Wire::Chat),
        ]
    );
}

/// `model-not-pulled` is the only refusal whose message carries more than its fixed text: every
/// other code's body is the fixed message alone, so the alias reaches no other refusal.
#[test]
fn ollama_no_other_refusal_names_the_alias() {
    let handle = gateway(Arc::new(Source::default()), true, |_| {});
    let answer = send(
        handle.local_addr(),
        "POST",
        "/v1/chat/completions",
        true,
        "{\"model\":\"small\"}",
    );
    handle.shutdown();
    assert_eq!(answer.status, 503, "{}", answer.body);
    assert_eq!(
        answer.body,
        format!(
            "{{\"error\":{{\"code\":\"target-unavailable\",\"message\":\"{}\"}}}}",
            RefusalCode::TargetUnavailable.reason()
        )
    );
}
