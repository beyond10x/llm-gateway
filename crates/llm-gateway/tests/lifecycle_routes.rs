//! The owner's lifecycle routes (story:model-wake-and-status): `POST /v1/models/{alias}/wake`
//! and `GET /v1/models/{alias}/status`, answered from what the injected [`RelayTargets`] says.
//! The specification is `llm-gateway.gateway.LifecycleRoute` in `spec/domains/gateway.yaml`;
//! the scenarios are `contracts/gateway/scenarios/wake-*`.
//!
//! The target source here is a script that records every ask; no target is ever handed out,
//! so no connection leaves the gateway at all.

use llm_gateway::{
    Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label, ModelPhase, ModelStatus, OwnerToken,
    RefusalCode, Relay, RelayModel, RelayTarget, RelayTargets, RouteInventory,
    SharedSecretVerifier, TargetRefusal, Wire,
};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
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
    let handle = gateway(Arc::clone(&source), true, |config| {
        config.ingress_auth = IngressAuth::None;
    });
    let address = handle.local_addr();
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
    ] {
        assert!(CONTRACT.contains(row), "docs/gateway.md has no row {row}");
    }
}
