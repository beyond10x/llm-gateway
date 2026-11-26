//! `IngressAuth::ServiceAccountToken` (story:gateway-sa-token-ingress): a model call admitted by
//! the owner's bearer or by the relay's `CallerReview`, every other path by the owner's bearer
//! alone, and `IngressAuth::Bearer` unchanged with a review composed. The review is a fake that
//! answers from a table and counts what it was asked; every target is a loopback pod.

use crate::upstream::{self, Upstream};
use llm_gateway::{
    CallerReview, CallerToken, Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label,
    OwnerToken, OwnerVerifier, Principal, Relay, RelayModel, RelayTarget, RelayTargets, Review,
    RouteInventory, SharedSecretVerifier, TargetRefusal, UsageRecord, UsageRecords, Wire,
};
use std::{
    fmt::Write as _,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ADMITTED: &str = "header.admitted-agent.signature";
const REFUSED: &str = "header.refused-agent.signature";
const UNREVIEWABLE: &str = "header.unreviewable.signature";
const AGENT: &str = "system:serviceaccount:agents:coding-agent";
const WAIT: Duration = Duration::from_secs(10);
const BODY: &str = r#"{"model":"open","messages":[]}"#;

/// A loopback pod answering every call of its test with `200 {}`, keeping every head it read. It
/// answers only its own test's calls (`crate::upstream`).
struct Pod {
    upstream: Upstream,
}

impl Pod {
    fn start() -> Self {
        Self {
            upstream: Upstream::answering(|connection| {
                drop(connection.write_all(upstream::OK));
            }),
        }
    }

    fn heads(&self) -> Vec<String> {
        self.upstream
            .seen()
            .iter()
            .map(|raw| upstream::head_of(raw))
            .collect()
    }
}

struct Source(upstream::Target);

impl RelayTargets for Source {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(self.0.clone()))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

#[derive(Default)]
struct Records(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Records {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

impl Records {
    fn principals(&self) -> Vec<Option<String>> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.principal.clone())
            .collect()
    }
}

/// Admits [`ADMITTED`] as [`AGENT`], cannot review [`UNREVIEWABLE`], rejects everything else,
/// and counts every review and keeps what it was shown.
#[derive(Debug, Default)]
struct FakeReview {
    asked: AtomicUsize,
    shown: Mutex<Vec<Vec<u8>>>,
}

impl CallerReview for FakeReview {
    fn review(&self, presented: &CallerToken) -> Review {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.shown.lock().unwrap().push(presented.expose().to_vec());
        match presented.expose() {
            token if token == ADMITTED.as_bytes() => {
                Review::Admitted(Principal::new(AGENT).unwrap())
            }
            token if token == UNREVIEWABLE.as_bytes() => Review::Unavailable,
            _ => Review::Rejected,
        }
    }
}

impl FakeReview {
    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

fn owner_verifier() -> Arc<dyn OwnerVerifier> {
    Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    )
}

struct Served {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
    pod: Pod,
    review: Arc<FakeReview>,
    records: Arc<Records>,
}

impl Served {
    /// A ready gateway under `ingress`, its relay composed with the fake review when `reviewed`.
    fn start(ingress: IngressAuth, reviewed: bool) -> Self {
        let pod = Pod::start();
        let review = Arc::new(FakeReview::default());
        let records = Arc::new(Records::default());
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = WAIT;
        config.ingress_auth = ingress;
        let model = RelayModel::new(
            Label::new("open").unwrap(),
            Label::new("served-open").unwrap(),
            vec![Wire::Chat, Wire::Responses, Wire::Messages],
        )
        .unwrap();
        let mut relay = Relay::new(vec![model], Arc::new(Source(pod.upstream.target(WAIT))))
            .unwrap()
            .with_records(Arc::clone(&records) as Arc<dyn UsageRecords>);
        if reviewed {
            relay = relay.with_caller_review(Arc::clone(&review) as Arc<dyn CallerReview>);
        }
        let handle = Gateway::bind_with_relay(
            config,
            owner_verifier(),
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
            pod,
            review,
            records,
        }
    }

    /// `(status, code)` of one request presenting `bearer`, or no `authorization` with `None`.
    fn ask(&self, method: &str, path: &str, bearer: Option<&str>, body: &str) -> (u16, String) {
        let mut raw = format!("{method} {path} HTTP/1.1\r\nhost: gateway\r\n");
        if let Some(bearer) = bearer {
            let _ = write!(raw, "authorization: Bearer {bearer}\r\n");
        }
        if !body.is_empty() {
            let _ = write!(
                raw,
                "content-type: application/json\r\ncontent-length: {}\r\n",
                body.len()
            );
        }
        raw.push_str("\r\n");
        raw.push_str(body);
        self.raw(&raw)
    }

    fn raw(&self, raw: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(self.addr).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        drop(stream.write_all(raw.as_bytes()));
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        let text = String::from_utf8_lossy(&answer).into_owned();
        let status = text
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .unwrap_or(0);
        let code = text
            .split("\"code\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or("relayed")
            .to_owned();
        (status, code)
    }

    fn chat(&self, bearer: Option<&str>) -> (u16, String) {
        self.ask("POST", "/v1/chat/completions", bearer, BODY)
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

#[test]
fn sa_a_reviewed_token_admits_a_model_call_and_its_record_names_the_principal() {
    let served = Served::start(IngressAuth::ServiceAccountToken, true);
    assert_eq!(served.chat(Some(ADMITTED)), (200, "relayed".to_owned()));
    assert_eq!(
        served.ask(
            "POST",
            "/v1/responses",
            Some(ADMITTED),
            r#"{"model":"open","input":"x"}"#
        ),
        (200, "relayed".to_owned())
    );
    assert_eq!(served.review.asked(), 2);
    assert_eq!(
        served.records.principals(),
        vec![Some(AGENT.to_owned()), Some(AGENT.to_owned())]
    );
    // The token reaches the review and never the target.
    assert!(
        served
            .pod
            .heads()
            .iter()
            .all(|head| !head.contains(ADMITTED))
    );
}

#[test]
fn sa_the_owners_bearer_still_admits_a_model_call_and_is_never_shown_to_the_review() {
    let served = Served::start(IngressAuth::ServiceAccountToken, true);
    assert_eq!(served.chat(Some(OWNER)), (200, "relayed".to_owned()));
    assert_eq!(
        served.review.asked(),
        0,
        "the owner's bearer went to the review"
    );
    assert_eq!(served.records.principals(), vec![None]);
}

#[test]
fn sa_each_failure_of_the_review_is_a_refusal_and_makes_no_record() {
    let served = Served::start(IngressAuth::ServiceAccountToken, true);
    assert_eq!(
        served.chat(Some(REFUSED)),
        (401, "credential-rejected".to_owned())
    );
    assert_eq!(
        served.chat(Some(UNREVIEWABLE)),
        (503, "credential-review-unavailable".to_owned())
    );
    assert_eq!(served.chat(None), (401, "credential-absent".to_owned()));
    assert_eq!(
        served.raw("POST /v1/chat/completions HTTP/1.1\r\nhost: g\r\nauthorization: Token x\r\ncontent-length: 2\r\n\r\n{}"),
        (401, "credential-malformed".to_owned())
    );
    assert_eq!(served.review.asked(), 2);
    assert!(served.records.principals().is_empty());
    assert!(
        served.pod.heads().is_empty(),
        "a refused call reached the target"
    );
}

#[test]
fn sa_without_a_review_every_caller_but_the_owner_is_unavailable() {
    let served = Served::start(IngressAuth::ServiceAccountToken, false);
    assert_eq!(
        served.chat(Some(ADMITTED)),
        (503, "credential-review-unavailable".to_owned())
    );
    assert_eq!(served.chat(Some(OWNER)), (200, "relayed".to_owned()));
}

#[test]
fn sa_a_reviewed_token_opens_no_owner_path_and_is_not_reviewed_for_one() {
    let served = Served::start(IngressAuth::ServiceAccountToken, true);
    for (method, path) in [
        ("GET", "/v1/routes"),
        ("GET", "/metrics"),
        ("GET", "/dashboard"),
        ("POST", "/v1/models/open/wake"),
        ("GET", "/v1/models/open/status"),
        ("GET", "/v1/nothing"),
    ] {
        assert_eq!(
            served.ask(method, path, Some(ADMITTED), ""),
            (401, "credential-rejected".to_owned()),
            "{method} {path}"
        );
    }
    assert_eq!(served.review.asked(), 0);
    assert_eq!(served.ask("GET", "/metrics", Some(OWNER), "").0, 200);
}

#[test]
fn sa_bearer_is_unchanged_when_a_review_is_composed() {
    let served = Served::start(IngressAuth::Bearer, true);
    assert_eq!(
        served.chat(Some(ADMITTED)),
        (401, "credential-rejected".to_owned())
    );
    assert_eq!(
        served.chat(Some(UNREVIEWABLE)),
        (401, "credential-rejected".to_owned())
    );
    assert_eq!(served.chat(Some(OWNER)), (200, "relayed".to_owned()));
    assert_eq!(
        served.review.asked(),
        0,
        "a bearer gateway asked the review"
    );
    assert_eq!(served.records.principals(), vec![None]);
}
