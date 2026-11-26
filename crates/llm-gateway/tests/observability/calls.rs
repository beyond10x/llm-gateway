//! The id of every model call and the record the owner reads by it
//! (story:call-id-and-usage-record; `CallHeader` and `CallRoute` in `spec/domains/gateway.yaml`,
//! `CallRecord` in `spec/domains/telemetry.yaml`, docs/gateway.md "Call ids and call records").
//!
//! Every target here is a fake upstream on loopback, port 0, which keeps the bytes of every
//! request it read and answers from a script; the gateway listens on loopback too. Nothing else
//! is opened.

use llm_gateway::{
    AnswerFraming, CallPricing, CallerReview, CallerToken, Gateway, GatewayConfig, GatewayHandle,
    IngressAuth, Label, OwnerToken, Principal, RefusalCode, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, Review, RouteInventory, SharedSecretVerifier, TargetRefusal,
    TranslatedAnswer, Translation, TranslationFailure, TranslationRequest, Translator, Usage,
    UsageRecord, UsageRecords, Wire,
};
use std::{
    collections::{BTreeSet, VecDeque},
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "other-token-0123456789abcdef0123456789abcdef";
const REVIEWED: &str = "header.reviewed-agent.signature";
const AGENT: &str = "system:serviceaccount:agents:coding-agent";
const HEADER: &str = "x-llm-gateway-call";
const WAIT: Duration = Duration::from_secs(10);
/// The number of ended calls docs/gateway.md publishes as `ended-calls-held`.
const HELD: usize = 1024;
/// One answer of the fake upstream: a chat completion reporting its usage.
const COMPLETION: &str = r#"{"id":"c","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18,"prompt_tokens_details":{"cached_tokens":3}}}"#;

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

// --- The fake upstream ---------------------------------------------------------------------------

/// What the fake upstream answers one connection with.
enum Reply {
    /// These raw bytes, written at once, then the connection closed.
    Whole(String),
    /// `first`, then `rest` once something is sent on (or the sender of) `release`.
    Held {
        first: String,
        rest: String,
        release: Receiver<()>,
    },
}

/// A loopback upstream answering each connection with the next scripted reply, and `200 {}`
/// past its script. It keeps the body bytes of every request it read, in order.
struct Upstream {
    authority: String,
    bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl Upstream {
    fn start(script: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&bodies);
        let script = Mutex::new(VecDeque::from(script));
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let Some(body) = read_request(&mut connection) else {
                    continue;
                };
                kept.lock().unwrap().push(body);
                let reply = script.lock().unwrap().pop_front();
                thread::spawn(move || answer(&mut connection, reply));
            }
        });
        Self { authority, bodies }
    }

    fn bodies(&self) -> Vec<Vec<u8>> {
        self.bodies.lock().unwrap().clone()
    }
}

/// The body of one request, read by its `content-length`.
fn read_request(connection: &mut TcpStream) -> Option<Vec<u8>> {
    connection.set_read_timeout(Some(WAIT)).ok()?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
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
    let mut body = buffer[end + 4..].to_vec();
    while body.len() < wanted {
        match connection.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => body.extend_from_slice(&chunk[..read]),
        }
    }
    Some(body)
}

fn answer(connection: &mut TcpStream, reply: Option<Reply>) {
    match reply {
        None => drop(connection.write_all(&json_answer("{}").into_bytes())),
        Some(Reply::Whole(raw)) => drop(connection.write_all(raw.as_bytes())),
        Some(Reply::Held {
            first,
            rest,
            release,
        }) => {
            drop(connection.write_all(first.as_bytes()));
            drop(connection.flush());
            let _released = release.recv_timeout(WAIT);
            drop(connection.write_all(rest.as_bytes()));
        }
    }
}

/// A `200` JSON answer framed by its length.
fn json_answer(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One chunk of a chunked answer.
fn chunk(text: &str) -> String {
    format!("{:x}\r\n{text}\r\n", text.len())
}

/// The head of a `200` event stream, chunked.
const STREAM_HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n";

struct Target(String);

impl RelayTarget for Target {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

/// Hands out the upstream for every alias.
struct Targets(String);

impl RelayTargets for Targets {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(Target(self.0.clone())))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// A translator that asks its target over loopback and writes the upstream's body as a JSON
/// answer on the client's wire.
struct Loopback;

struct LoopbackRun;

impl Translator for Loopback {
    fn prepare(
        &self,
        _request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        Ok(Box::new(LoopbackRun))
    }
}

impl Translation for LoopbackRun {
    fn run(
        self: Box<Self>,
        target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure> {
        let failed = |_| TranslationFailure::UpstreamFailed;
        let mut stream = target.connect().map_err(failed)?;
        stream
            .write_all(b"POST /v1/chat/completions HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}")
            .map_err(failed)?;
        let mut raw = Vec::new();
        drop(stream.read_to_end(&mut raw));
        let text = String::from_utf8_lossy(&raw).into_owned();
        let (_, body) = text
            .split_once("\r\n\r\n")
            .ok_or(TranslationFailure::UpstreamFailed)?;
        answer.target_status(200);
        if answer.head(
            200,
            "application/json",
            AnswerFraming::Length(u64::try_from(body.len()).unwrap()),
        ) {
            answer.body(body.as_bytes());
        }
        Ok(())
    }
}

/// Admits [`REVIEWED`] as [`AGENT`] and rejects every other bearer.
#[derive(Debug)]
struct Reviews;

impl CallerReview for Reviews {
    fn review(&self, presented: &CallerToken) -> Review {
        if presented.expose() == REVIEWED.as_bytes() {
            Review::Admitted(Principal::new(AGENT).unwrap())
        } else {
            Review::Rejected
        }
    }
}

/// Keeps every usage record the gateway hands out.
#[derive(Default)]
struct Kept(Mutex<Vec<UsageRecord>>);

impl UsageRecords for Kept {
    fn record(&self, record: &UsageRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

// --- The gateway ---------------------------------------------------------------------------------

struct Fixture {
    handle: GatewayHandle,
    kept: Arc<Kept>,
}

impl Fixture {
    fn address(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    fn records(&self) -> Vec<UsageRecord> {
        self.kept.0.lock().unwrap().clone()
    }
}

/// `code`, relayed on chat and responses and translated on messages, whose upstream name is its
/// alias, so a body naming it reaches the upstream unrewritten.
fn code() -> RelayModel {
    RelayModel::new(
        label("code"),
        label("code"),
        vec![Wire::Chat, Wire::Responses],
    )
    .unwrap()
}

/// A ready gateway on loopback relaying `models` to `upstream`, configured by `configure`.
fn gateway(
    upstream: &Upstream,
    models: Vec<RelayModel>,
    configure: impl FnOnce(&mut GatewayConfig),
) -> Fixture {
    let kept = Arc::new(Kept::default());
    let relay = Relay::new(models, Arc::new(Targets(upstream.authority.clone())))
        .unwrap()
        .with_translator(Arc::new(Loopback))
        .with_caller_review(Arc::new(Reviews))
        .with_records(Arc::clone(&kept) as Arc<dyn UsageRecords>);
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    configure(&mut config);
    let verifier =
        Arc::new(SharedSecretVerifier::new(OwnerToken::new(OWNER.into()).unwrap()).unwrap());
    let handle = Gateway::bind_with_relay(
        config,
        verifier,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    Fixture { handle, kept }
}

// --- The client ----------------------------------------------------------------------------------

#[derive(Debug)]
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Answer {
    /// Every value of the header `name`, in order.
    fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.all(name).first().copied()
    }

    /// The call id the answer carried: exactly one `x-llm-gateway-call`, or none.
    fn call(&self) -> Option<String> {
        let ids = self.all(HEADER);
        assert!(ids.len() <= 1, "more than one call id: {self:?}");
        ids.first().map(|id| (*id).to_owned())
    }
}

/// Parses an answer read to its end.
fn parse(raw: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(raw).into_owned();
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no head in {text:?}"));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(name, value)| name == "transfer-encoding" && value == "chunked");
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_owned()
    };
    Answer {
        status,
        headers,
        body,
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

fn send(address: SocketAddr, raw: &str) -> Answer {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    parse(&answer)
}

fn bearer(credential: Option<&str>) -> String {
    credential.map_or_else(String::new, |token| {
        format!("authorization: Bearer {token}\r\n")
    })
}

fn post(address: SocketAddr, path: &str, body: &str, credential: Option<&str>) -> Answer {
    send(
        address,
        &format!(
            "POST {path} HTTP/1.1\r\nhost: g\r\n{}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            bearer(credential),
            body.len()
        ),
    )
}

fn read(address: SocketAddr, method: &str, path: &str, credential: Option<&str>) -> Answer {
    send(
        address,
        &format!(
            "{method} {path} HTTP/1.1\r\nhost: g\r\n{}\r\n",
            bearer(credential)
        ),
    )
}

/// The owner's read of the record of call `id`.
fn record(address: SocketAddr, id: &str) -> Answer {
    read(address, "GET", &format!("/v1/calls/{id}"), Some(OWNER))
}

/// Whether `id` has the published form: 1 to 64 of `A-Z a-z 0-9 -`.
fn of_the_published_form(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// `body` with its `"duration_ms":<n>` written `"duration_ms":D`, and `n`.
fn timed(body: &str) -> (String, u64) {
    let key = "\"duration_ms\":";
    let at = body
        .find(key)
        .unwrap_or_else(|| panic!("no duration in {body}"))
        + key.len();
    let digits: String = body[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let value = digits.parse().unwrap();
    (
        format!("{}D{}", &body[..at], &body[at + digits.len()..]),
        value,
    )
}

const CALL_UNKNOWN: &str =
    r#"{"error":{"code":"call-unknown","message":"the gateway holds no call with that id"}}"#;

// --- Acceptance 1: the header ------------------------------------------------------------------

/// A relayed answer, a translated one, a refusal before the body is read and one after it each
/// carry exactly one id of the published form, and no two the same.
#[test]
fn a1_every_answer_to_an_authenticated_model_call_carries_one_id_of_the_published_form() {
    let upstream = Upstream::start(vec![
        Reply::Whole(json_answer(COMPLETION)),
        Reply::Whole(json_answer(r#"{"translated":true}"#)),
    ]);
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let relayed = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    );
    let translated = post(address, "/v1/messages", r#"{"model":"code"}"#, Some(OWNER));
    let unknown = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"nobody"}"#,
        Some(OWNER),
    );
    let wrong_method = read(address, "GET", "/v1/responses", Some(OWNER));
    assert_eq!(relayed.status, 200, "{relayed:?}");
    assert_eq!(relayed.body, COMPLETION);
    assert_eq!(translated.status, 200, "{translated:?}");
    assert_eq!(translated.body, r#"{"translated":true}"#);
    assert_eq!(unknown.status, 404, "{unknown:?}");
    assert_eq!(wrong_method.status, 405, "{wrong_method:?}");
    let answers = [&relayed, &translated, &unknown, &wrong_method];
    let ids: Vec<String> = answers
        .iter()
        .map(|answer| {
            answer
                .call()
                .unwrap_or_else(|| panic!("no call id: {answer:?}"))
        })
        .collect();
    for id in &ids {
        assert!(of_the_published_form(id), "{id:?}");
    }
    let distinct: BTreeSet<&String> = ids.iter().collect();
    assert_eq!(distinct.len(), ids.len(), "{ids:?}");
    // Each usage record carries the id its answer did.
    let recorded: Vec<String> = fixture
        .records()
        .iter()
        .map(|record| record.call.clone())
        .collect();
    assert_eq!(recorded, ids);
}

/// A model marked to take the client's own credential, behind a gateway that authenticates
/// nobody: the passthrough answer carries the gateway's id, and the upstream's own line of that
/// name never reaches the client.
#[test]
fn a1_a_passthrough_answer_carries_the_gateways_id_and_never_the_upstreams() {
    let upstream = Upstream::start(vec![Reply::Whole(format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n{HEADER}: forged-by-the-upstream\r\nrequest-id: req_1\r\ncontent-length: 2\r\n\r\n{{}}"
    ))]);
    let model = RelayModel::new(label("claude"), label("claude"), vec![Wire::Messages])
        .unwrap()
        .with_client_credentials();
    let fixture = gateway(&upstream, vec![model], |config| {
        config.ingress_auth = IngressAuth::None;
    });
    let answered = send(
        fixture.address(),
        "POST /v1/messages HTTP/1.1\r\nhost: g\r\nx-api-key: client-key\r\ncontent-type: application/json\r\ncontent-length: 18\r\n\r\n{\"model\":\"claude\"}",
    );
    assert_eq!(answered.status, 200, "{answered:?}");
    assert_eq!(answered.header("request-id"), Some("req_1"));
    let id = answered.call().expect("a passthrough answer carries an id");
    assert!(of_the_published_form(&id), "{id:?}");
    assert_ne!(id, "forged-by-the-upstream");
    assert_eq!(fixture.records()[0].call, id);
}

/// A request shed for load gets no id and makes no record.
#[test]
fn a1_a_shed_request_gets_no_id_and_no_record() {
    let upstream = Upstream::start(Vec::new());
    let fixture = gateway(&upstream, vec![code()], |config| {
        config.max_concurrent_requests = 0;
    });
    let shed = post(
        fixture.address(),
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    );
    assert_eq!(shed.status, 503, "{shed:?}");
    assert!(shed.body.contains("\"overloaded\""), "{shed:?}");
    assert_eq!(shed.call(), None, "{shed:?}");
    assert!(fixture.records().is_empty());
    assert!(upstream.bodies().is_empty());
}

/// A request that fails authentication, or its review, gets no id and makes no record.
#[test]
fn a1_a_request_that_fails_authentication_gets_no_id_and_no_record() {
    let upstream = Upstream::start(Vec::new());
    for ingress in [IngressAuth::Bearer, IngressAuth::ServiceAccountToken] {
        let fixture = gateway(&upstream, vec![code()], |config| {
            config.ingress_auth = ingress;
        });
        let address = fixture.address();
        for credential in [None, Some(OTHER)] {
            let refused = post(
                address,
                "/v1/chat/completions",
                r#"{"model":"code"}"#,
                credential,
            );
            assert_eq!(
                refused.status, 401,
                "{ingress:?} {credential:?}: {refused:?}"
            );
            assert_eq!(
                refused.call(),
                None,
                "{ingress:?} {credential:?}: {refused:?}"
            );
        }
        assert!(fixture.records().is_empty(), "{ingress:?}");
    }
    assert!(upstream.bodies().is_empty());
}

/// Within one process a gateway composed again gives out ids its earlier composition never
/// gave, and holds none of the earlier ones.
#[test]
fn a1_a_gateway_composed_again_gives_out_no_earlier_id_and_holds_none() {
    let upstream = Upstream::start(Vec::new());
    let first = gateway(&upstream, vec![code()], |_| {});
    let before = post(
        first.address(),
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    )
    .call()
    .unwrap();
    first.handle.shutdown();
    let second = gateway(&upstream, vec![code()], |_| {});
    let after = post(
        second.address(),
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    )
    .call()
    .unwrap();
    assert_ne!(before, after);
    let earlier = record(second.address(), &before);
    assert_eq!(earlier.status, 404, "{earlier:?}");
    assert_eq!(earlier.body, CALL_UNKNOWN);
}

// --- Acceptance 2: the record ------------------------------------------------------------------

/// A relayed call's record names it, its tokens under the story's keys in the story's order,
/// and leaves out what the usage record does not carry; a refused call's names its refusal and
/// carries no token key and no target status.
#[test]
fn a2_the_owner_reads_an_ended_calls_record_by_its_id_with_its_keys_in_order() {
    let upstream = Upstream::start(vec![Reply::Whole(json_answer(COMPLETION))]);
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let started = Instant::now();
    let relayed = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    );
    let refused = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"nobody"}"#,
        Some(OWNER),
    );
    let waited = u64::try_from(started.elapsed().as_millis()).unwrap();
    let relayed_id = relayed.call().unwrap();
    let refused_id = refused.call().unwrap();

    let read = record(address, &relayed_id);
    assert_eq!(read.status, 200, "{read:?}");
    assert_eq!(read.header("content-type"), Some("application/json"));
    assert_eq!(read.header("cache-control"), Some("no-store"));
    assert_eq!(read.call(), None, "a record's own answer is no model call");
    let (body, duration) = timed(&read.body);
    assert_eq!(
        body,
        format!(
            r#"{{"format":"llm-gateway.call/1","id":"{relayed_id}","state":"ended","model":"code","wire":"chat","disposition":"relayed","outcome":"ok","status":200,"target_status":200,"input_tokens":11,"output_tokens":7,"cache_read_tokens":3,"response_bytes":{},"duration_ms":D}}"#,
            COMPLETION.len()
        )
    );
    assert!(
        duration <= waited,
        "{duration} ms, the client waited {waited} ms"
    );

    let read = record(address, &refused_id);
    assert_eq!(read.status, 200, "{read:?}");
    let (body, _) = timed(&read.body);
    assert_eq!(
        body,
        format!(
            r#"{{"format":"llm-gateway.call/1","id":"{refused_id}","state":"ended","wire":"chat","disposition":"refused","outcome":"refused","refusal":"model-unknown","status":404,"response_bytes":0,"duration_ms":D}}"#
        )
    );
    // `HEAD` answers the same head without the body.
    let head = read_head_only(address, &refused_id);
    assert_eq!(head.status, 200, "{head:?}");
    assert_eq!(head.body, "");
    assert_eq!(
        head.header("content-length"),
        Some(read.body.len().to_string().as_str())
    );
}

/// A pricing whose answer is made of the counters it was asked about, so a record shows which
/// usage reached it: `0.<input><output><cache read>`, none when one of them is unreported.
struct Echo;

impl CallPricing for Echo {
    fn cost_usd(&self, _wire: Wire, usage: Usage) -> Option<String> {
        Some(format!(
            "0.{}{}{}",
            usage.input_tokens?, usage.output_tokens?, usage.cache_read_tokens?
        ))
    }
}

/// story:cost-per-call: a priced model's record names `cost_usd` after its token keys, what the
/// model's pricing answered for the tokens the fake reported; the same call of a model that
/// declares no pricing has no such key.
#[test]
fn a2_a_priced_calls_record_names_its_cost_after_the_token_keys_and_an_unpriced_ones_none() {
    let upstream = Upstream::start(vec![
        Reply::Whole(json_answer(COMPLETION)),
        Reply::Whole(json_answer(COMPLETION)),
    ]);
    let priced = RelayModel::new(label("priced"), label("priced"), vec![Wire::Chat])
        .unwrap()
        .with_pricing(Arc::new(Echo));
    let fixture = gateway(&upstream, vec![code(), priced], |_| {});
    let address = fixture.address();
    let ids: Vec<String> = ["priced", "code"]
        .into_iter()
        .map(|alias| {
            post(
                address,
                "/v1/chat/completions",
                &format!(r#"{{"model":"{alias}"}}"#),
                Some(OWNER),
            )
            .call()
            .unwrap()
        })
        .collect();
    let read = record(address, &ids[0]);
    assert_eq!(read.status, 200, "{read:?}");
    let (body, _) = timed(&read.body);
    assert_eq!(
        body,
        format!(
            r#"{{"format":"llm-gateway.call/1","id":"{}","state":"ended","model":"priced","wire":"chat","disposition":"relayed","outcome":"ok","status":200,"target_status":200,"input_tokens":11,"output_tokens":7,"cache_read_tokens":3,"cost_usd":"0.1173","response_bytes":{},"duration_ms":D}}"#,
            ids[0],
            COMPLETION.len()
        )
    );
    let read = record(address, &ids[1]);
    assert_eq!(read.status, 200, "{read:?}");
    assert!(!read.body.contains("cost_usd"), "{}", read.body);
    assert!(
        read.body
            .contains("\"cache_read_tokens\":3,\"response_bytes\":"),
        "{}",
        read.body
    );
}

fn read_head_only(address: SocketAddr, id: &str) -> Answer {
    read(address, "HEAD", &format!("/v1/calls/{id}"), Some(OWNER))
}

/// A reviewed service account's call names its principal; the record of that call is still the
/// owner's to read.
#[test]
fn a2_a_reviewed_callers_record_names_its_principal_and_only_the_owner_reads_it() {
    let upstream = Upstream::start(vec![Reply::Whole(json_answer("{}"))]);
    let fixture = gateway(&upstream, vec![code()], |config| {
        config.ingress_auth = IngressAuth::ServiceAccountToken;
    });
    let address = fixture.address();
    let called = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(REVIEWED),
    );
    assert_eq!(called.status, 200, "{called:?}");
    let id = called.call().unwrap();
    let by_agent = read(address, "GET", &format!("/v1/calls/{id}"), Some(REVIEWED));
    assert_eq!(by_agent.status, 401, "{by_agent:?}");
    let read = record(address, &id);
    assert_eq!(read.status, 200, "{read:?}");
    assert!(
        read.body
            .ends_with(&format!(",\"principal\":\"{AGENT}\"}}")),
        "{}",
        read.body
    );
}

/// The record takes the credential `GET /v1/launch` takes, under every `IngressAuth` and on a
/// gateway open on loopback: each credential is answered by the record route as the launch
/// route answers it.
#[test]
fn a2_the_record_takes_exactly_the_credential_the_launch_route_takes() {
    let upstream = Upstream::start(Vec::new());
    for ingress in [
        IngressAuth::Bearer,
        IngressAuth::None,
        IngressAuth::ServiceAccountToken,
    ] {
        for bind in ["127.0.0.1:0", "0.0.0.0:0"] {
            let fixture = gateway(&upstream, vec![code()], |config| {
                config.ingress_auth = ingress;
                config.bind = bind.parse().unwrap();
            });
            let address = SocketAddr::from(([127, 0, 0, 1], fixture.address().port()));
            let id = post(
                address,
                "/v1/chat/completions",
                r#"{"model":"code"}"#,
                Some(OWNER),
            )
            .call()
            .unwrap_or_else(|| panic!("{ingress:?} {bind}: the owner's call has no id"));
            for credential in [None, Some(OWNER), Some(OTHER), Some(REVIEWED)] {
                let launch = read(address, "GET", "/v1/launch", credential);
                let call = read(address, "GET", &format!("/v1/calls/{id}"), credential);
                assert_eq!(
                    call.status, launch.status,
                    "{ingress:?} {bind} {credential:?}: {call:?} against {launch:?}"
                );
                if launch.status != 200 {
                    assert_eq!(call.body, launch.body, "{ingress:?} {bind} {credential:?}");
                }
            }
        }
    }
}

/// Another method is `method-not-allowed` and a declared body `body-not-allowed`, before any
/// call is looked up.
#[test]
fn a2_the_record_takes_get_and_head_and_no_body() {
    let upstream = Upstream::start(Vec::new());
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let id = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    )
    .call()
    .unwrap();
    let posted = post(address, &format!("/v1/calls/{id}"), "", Some(OWNER));
    assert_eq!(posted.status, 405, "{posted:?}");
    assert_eq!(posted.header("allow"), Some("GET, HEAD"));
    let with_body = send(
        address,
        &format!(
            "GET /v1/calls/{id} HTTP/1.1\r\nhost: g\r\n{}content-length: 2\r\n\r\n{{}}",
            bearer(Some(OWNER))
        ),
    );
    assert_eq!(with_body.status, 400, "{with_body:?}");
    assert!(with_body.body.contains("\"body-not-allowed\""));
}

// --- Acceptance 3: in flight, then ended ---------------------------------------------------------

/// While the upstream holds its answer open the record is `in-flight` with what is known; once
/// the last byte is written it is `ended` with the usage the answer reported.
#[test]
fn a3_a_call_is_in_flight_while_its_answer_is_held_open_and_ended_after_its_last_byte() {
    let (release, released): (Sender<()>, Receiver<()>) = mpsc::channel();
    let first = format!("{STREAM_HEAD}{}", chunk("data: {\"choices\":[]}\n\n"));
    let rest = format!(
        "{}{}0\r\n\r\n",
        chunk("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n"),
        chunk("data: [DONE]\n\n")
    );
    let upstream = Upstream::start(vec![Reply::Held {
        first,
        rest,
        release: released,
    }]);
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let body = r#"{"model":"code","stream":true,"stream_options":{"include_usage":true}}"#;
    let mut client = TcpStream::connect(address).unwrap();
    client.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        client,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: g\r\n{}content-length: {}\r\n\r\n{body}",
        bearer(Some(OWNER)),
        body.len()
    )
    .unwrap();
    // Read until the first event has arrived: the head, with the id, is in.
    let mut raw = Vec::new();
    let mut piece = [0_u8; 4096];
    while !String::from_utf8_lossy(&raw).contains("data: {\"choices\":[]}") {
        let read = client.read(&mut piece).unwrap();
        assert!(read > 0, "the answer ended early: {raw:?}");
        raw.extend_from_slice(&piece[..read]);
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let id = head
        .split("\r\n")
        .find_map(|line| line.strip_prefix(&format!("{HEADER}: ")))
        .expect("the streamed answer's head carries the id")
        .to_owned();

    let during = record(address, &id);
    assert_eq!(during.status, 200, "{during:?}");
    assert_eq!(
        during.body,
        format!(
            r#"{{"format":"llm-gateway.call/1","id":"{id}","state":"in-flight","model":"code","wire":"chat"}}"#
        )
    );

    release.send(()).unwrap();
    let mut rest = Vec::new();
    drop(client.read_to_end(&mut rest));
    raw.extend_from_slice(&rest);
    let answered = parse(&raw);
    assert!(answered.body.ends_with("data: [DONE]\n\n"), "{answered:?}");

    let after = record(address, &id);
    let (body, _) = timed(&after.body);
    assert_eq!(
        body,
        format!(
            r#"{{"format":"llm-gateway.call/1","id":"{id}","state":"ended","model":"code","wire":"chat","disposition":"relayed","outcome":"ok","status":200,"target_status":200,"input_tokens":5,"output_tokens":2,"response_bytes":{},"duration_ms":D}}"#,
            answered.body.len()
        )
    );
}

// --- Acceptance 4: what is held ------------------------------------------------------------------

/// An id of another form and one never given out are refused alike, and the answer does not
/// say which.
#[test]
fn a4_an_id_of_another_form_or_never_given_out_is_call_unknown() {
    let upstream = Upstream::start(Vec::new());
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let given = post(
        address,
        "/v1/chat/completions",
        r#"{"model":"code"}"#,
        Some(OWNER),
    )
    .call()
    .unwrap();
    // The same form as a given id, with a counter the process never reaches.
    let (part, _) = given.rsplit_once('-').unwrap();
    let never = format!("{part}-0");
    for id in [
        never.as_str(),
        "not-an-id-at-all",
        "a.b",
        &"a".repeat(65),
        "",
        &format!("{given}/more"),
    ] {
        let refused = record(address, id);
        assert_eq!(refused.status, 404, "{id:?}: {refused:?}");
        assert_eq!(refused.body, CALL_UNKNOWN, "{id:?}");
        assert_eq!(refused.call(), None, "{id:?}");
    }
    assert_eq!(record(address, &given).status, 200);
}

/// Ends `count` calls (each refused `model-unknown`, which asks no target): the first and second
/// one after the other, the rest from eight clients at once. Returns the first two ids.
fn end_calls(address: SocketAddr, count: usize) -> (String, String) {
    let one = || {
        post(
            address,
            "/v1/chat/completions",
            r#"{"model":"nobody"}"#,
            Some(OWNER),
        )
        .call()
        .unwrap()
    };
    let first = one();
    let second = one();
    let rest = count - 2;
    let clients = 8;
    let workers: Vec<_> = (0..clients)
        .map(|index| {
            let share = rest / clients + usize::from(index < rest % clients);
            thread::spawn(move || {
                for _ in 0..share {
                    let answered = post(
                        address,
                        "/v1/chat/completions",
                        r#"{"model":"nobody"}"#,
                        Some(OWNER),
                    );
                    assert_eq!(answered.status, 404, "{answered:?}");
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    (first, second)
}

/// The gateway holds the last 1024 ended calls: after 1025 the first is `call-unknown` and the
/// second is held.
#[test]
fn a4_after_1025_ended_calls_the_first_is_call_unknown_and_the_second_is_held() {
    let upstream = Upstream::start(Vec::new());
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let (first, second) = end_calls(address, HELD + 1);
    assert_eq!(fixture.records().len(), HELD + 1);
    let dropped = record(address, &first);
    assert_eq!(dropped.status, 404, "{dropped:?}");
    assert_eq!(dropped.body, CALL_UNKNOWN);
    let held = record(address, &second);
    assert_eq!(held.status, 200, "{held:?}");
    assert!(held.body.contains("\"state\":\"ended\""), "{}", held.body);
}

// --- Acceptance 6: a chat stream without usage ---------------------------------------------------

/// A streamed chat call whose client did not ask for usage is relayed with its body unchanged
/// and has no token counter; one whose client asked (`stream_options.include_usage`) has them.
#[test]
fn a6_a_streamed_chat_call_whose_client_asked_no_usage_is_relayed_unchanged_and_has_no_tokens() {
    let events = |usage: bool| {
        let last = if usage {
            chunk(
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n",
            )
        } else {
            String::new()
        };
        format!(
            "{STREAM_HEAD}{}{last}{}0\r\n\r\n",
            chunk("data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n"),
            chunk("data: [DONE]\n\n")
        )
    };
    let upstream = Upstream::start(vec![
        Reply::Whole(events(false)),
        Reply::Whole(events(true)),
    ]);
    let fixture = gateway(&upstream, vec![code()], |_| {});
    let address = fixture.address();
    let plain = r#"{"model":"code","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;
    let asked = r#"{"model":"code","stream":true,"stream_options":{"include_usage":true},"messages":[{"role":"user","content":"hi"}]}"#;
    let without = post(address, "/v1/chat/completions", plain, Some(OWNER));
    let with = post(address, "/v1/chat/completions", asked, Some(OWNER));
    assert_eq!(
        upstream.bodies(),
        vec![plain.as_bytes().to_vec(), asked.as_bytes().to_vec()],
        "each body reaches the upstream byte for byte"
    );
    let without = record(address, &without.call().unwrap());
    for key in [
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_creation_tokens",
    ] {
        assert!(!without.body.contains(key), "{key}: {}", without.body);
    }
    let with = record(address, &with.call().unwrap());
    assert!(
        with.body
            .contains("\"input_tokens\":9,\"output_tokens\":4,\"response_bytes\":"),
        "{}",
        with.body
    );
}

// --- Adversary pass, wave 2026-11-25-w03 ----------------------------------------------------------

/// Cases of the adversary pass on the joined wave (story:cost-per-call and
/// story:call-id-and-usage-record): what a call's record and its cost say when the answer did
/// not end as written, and which counts reach the pricing. Every target is the fake upstream on
/// loopback, port 0.
mod adversary_w03 {
    use super::*;

    /// A pricing that answers a cost for every call whose answer reported its input and output
    /// tokens, and keeps what it was asked: a cost in the record is a cost the gateway asked for.
    #[derive(Default)]
    struct Asked(Mutex<Vec<Usage>>);

    impl CallPricing for Asked {
        fn cost_usd(&self, _wire: Wire, usage: Usage) -> Option<String> {
            self.0.lock().unwrap().push(usage);
            usage.input_tokens?;
            usage.output_tokens?;
            Some("0.5".to_owned())
        }
    }

    /// `priced` on the messages wire, priced by `pricing`.
    fn priced(pricing: &Arc<Asked>) -> RelayModel {
        RelayModel::new(label("priced"), label("priced"), vec![Wire::Messages])
            .unwrap()
            .with_pricing(Arc::clone(pricing) as Arc<dyn CallPricing>)
    }

    /// A messages stream's first two events: `message_start`, which reports the input tokens
    /// and the first output token, and one text delta.
    fn started_stream() -> String {
        format!(
            "{STREAM_HEAD}{}{}",
            chunk(
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n"
            ),
            chunk(
                "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n"
            ),
        )
    }

    /// The raw answer bytes of one request, read to the close, unparsed.
    fn raw(address: SocketAddr, request: &str) -> String {
        let mut stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = Vec::new();
        drop(stream.read_to_end(&mut answer));
        String::from_utf8_lossy(&answer).into_owned()
    }

    /// docs/gateway.md "The cost of a call": `cost_usd` is present only when "the call was
    /// relayed and its answer ended as written"; `UsageRecord::cost_usd`: "`None` for a call
    /// that was refused, cut short or ended in its wire's terminal error". docs/gateway.md, relay
    /// step 9: "An answer cut short on either side ends without its last chunk". Here the
    /// upstream closes a streamed messages answer after `message_start` and one delta: the
    /// client gets the answer without its last chunk, and the record must carry no cost.
    #[test]
    fn adv_w03_a_stream_the_upstream_cuts_short_has_no_cost() {
        let upstream = Upstream::start(vec![Reply::Whole(started_stream())]);
        let pricing = Arc::new(Asked::default());
        let fixture = gateway(&upstream, vec![priced(&pricing)], |_| {});
        let address = fixture.address();
        let body = r#"{"model":"priced","stream":true}"#;
        let answered = raw(
            address,
            &format!(
                "POST /v1/messages HTTP/1.1\r\nhost: g\r\n{}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                bearer(Some(OWNER)),
                body.len()
            ),
        );
        assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
        assert!(
            !answered.ends_with("0\r\n\r\n"),
            "the client was shown the answer cut short: {answered:?}"
        );
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            records[0].cost_usd,
            None,
            "a stream the upstream cut short after message_start was priced: {:?}; the pricing was asked {:?}",
            records[0],
            pricing.0.lock().unwrap()
        );
        let read = record(address, &records[0].call);
        assert!(!read.body.contains("cost_usd"), "{}", read.body);
    }

    /// The same cut on a client-credential route (passthrough): the gateway relays the upstream's
    /// answer as it came, and an answer the upstream cut short ended no more as written there.
    #[test]
    fn adv_w03_a_passthrough_stream_the_upstream_cuts_short_has_no_cost() {
        let upstream = Upstream::start(vec![Reply::Whole(started_stream())]);
        let pricing = Arc::new(Asked::default());
        let model = priced(&pricing).with_client_credentials();
        let fixture = gateway(&upstream, vec![model], |config| {
            config.ingress_auth = IngressAuth::None;
        });
        let body = r#"{"model":"priced","stream":true}"#;
        let answered = raw(
            fixture.address(),
            &format!(
                "POST /v1/messages HTTP/1.1\r\nhost: g\r\nx-api-key: client-key\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(answered.starts_with("HTTP/1.1 200 "), "{answered}");
        assert!(
            !answered.ends_with("0\r\n\r\n"),
            "the client was shown the answer cut short: {answered:?}"
        );
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            records[0].cost_usd,
            None,
            "a passthrough stream the upstream cut short was priced: {:?}; the pricing was asked {:?}",
            records[0],
            pricing.0.lock().unwrap()
        );
    }

    /// `UsageRecord::cost_usd`: "`None` for a call that ... ended in its wire's terminal error".
    /// A relayed messages stream that the upstream ends with the wire's `error` event (an
    /// `overloaded_error` after output) and a well-framed last chunk ended in its wire's
    /// terminal error, and has no cost.
    #[test]
    fn adv_w03_a_relayed_stream_ended_by_the_wires_error_event_has_no_cost() {
        let ended = format!(
            "{}{}0\r\n\r\n",
            started_stream(),
            chunk(
                "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n"
            )
        );
        let upstream = Upstream::start(vec![Reply::Whole(ended)]);
        let pricing = Arc::new(Asked::default());
        let fixture = gateway(&upstream, vec![priced(&pricing)], |_| {});
        let answered = post(
            fixture.address(),
            "/v1/messages",
            r#"{"model":"priced","stream":true}"#,
            Some(OWNER),
        );
        assert_eq!(answered.status, 200, "{answered:?}");
        assert!(answered.body.contains("overloaded_error"), "{answered:?}");
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(
            records[0].cost_usd, None,
            "a stream ended by the wire's error event was priced: {:?}",
            records[0]
        );
    }

    /// A pricing that writes the counts it was asked about into its answer: `0.<input><output>`.
    struct Counts;

    impl CallPricing for Counts {
        fn cost_usd(&self, _wire: Wire, usage: Usage) -> Option<String> {
            Some(format!("0.{}{}", usage.input_tokens?, usage.output_tokens?))
        }
    }

    /// docs/gateway.md "Counters and usage records": the token counters "come from the usage the
    /// answer carried"; story:cost-per-call Acceptance 2 prices "a call whose ... answer
    /// reported tokens". A non-streamed messages answer whose model called a tool with
    /// arguments named `input_tokens` and `output_tokens` reported 20 input and 5 output
    /// tokens in its `usage`; the tool's arguments are no usage, and neither the record nor
    /// the cost may read them.
    #[test]
    fn adv_w03_a_tool_calls_arguments_are_not_the_answers_usage_and_are_not_priced() {
        let answer = r#"{"id":"m","type":"message","role":"assistant","model":"priced","content":[{"type":"tool_use","id":"toolu_1","name":"set_budget","input":{"input_tokens":900000,"output_tokens":800000}}],"stop_reason":"tool_use","usage":{"input_tokens":20,"output_tokens":5}}"#;
        let upstream = Upstream::start(vec![Reply::Whole(json_answer(answer))]);
        let model = RelayModel::new(label("priced"), label("priced"), vec![Wire::Messages])
            .unwrap()
            .with_pricing(Arc::new(Counts));
        let fixture = gateway(&upstream, vec![model], |_| {});
        let answered = post(
            fixture.address(),
            "/v1/messages",
            r#"{"model":"priced"}"#,
            Some(OWNER),
        );
        assert_eq!(answered.status, 200, "{answered:?}");
        assert_eq!(answered.body, answer);
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        let counted = (
            records[0].input_tokens,
            records[0].output_tokens,
            records[0].cost_usd.as_deref(),
        );
        assert_eq!(
            counted,
            (Some(20), Some(5), Some("0.205")),
            "the usage the answer reported is 20 input and 5 output tokens: {:?}",
            records[0]
        );
    }
}

/// Two cases of the same adversary pass that hold the two other conditions of a call's cost,
/// which no case of the suite held: each is green on the tree it was written against, and red
/// when its condition is dropped from the record (`crates/llm-gateway/src/server.rs`, `priced`).
mod adversary_w03_cost_conditions {
    use super::*;

    /// A messages `message_start` reporting 12 input tokens and the first output token.
    const STARTED: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n";

    /// A pricing that answers a cost whenever input and output were reported.
    struct Half;

    impl CallPricing for Half {
        fn cost_usd(&self, _wire: Wire, usage: Usage) -> Option<String> {
            usage.input_tokens?;
            usage.output_tokens?;
            Some("0.5".to_owned())
        }
    }

    /// What the translation does after the head and `message_start` are written.
    #[derive(Clone, Copy)]
    enum Then {
        /// Ends the answer in its wire's terminal error (`TranslatedAnswer::failed`).
        Fails,
        /// Waits until the client has closed its connection, then ends.
        WaitsForTheClientToLeave,
    }

    struct Scripted(Then);

    struct ScriptedRun(Then);

    impl Translator for Scripted {
        fn prepare(
            &self,
            _request: &TranslationRequest<'_>,
        ) -> Result<Box<dyn Translation>, RefusalCode> {
            Ok(Box::new(ScriptedRun(self.0)))
        }
    }

    impl Translation for ScriptedRun {
        fn run(
            self: Box<Self>,
            _target: &dyn RelayTarget,
            answer: &mut dyn TranslatedAnswer,
        ) -> Result<(), TranslationFailure> {
            answer.target_status(200);
            if !answer.head(200, "text/event-stream", AnswerFraming::Stream)
                || !answer.body(STARTED.as_bytes())
            {
                return Ok(());
            }
            match self.0 {
                Then::Fails => {
                    answer.failed(RefusalCode::UpstreamFailed);
                    Err(TranslationFailure::UpstreamFailed)
                }
                Then::WaitsForTheClientToLeave => {
                    let deadline = Instant::now() + WAIT;
                    while !answer.gone() && Instant::now() < deadline {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Ok(())
                }
            }
        }
    }

    /// A gateway whose `priced` model is served on chat, priced by [`Half`], and translated by
    /// `then` on the messages wire.
    fn translating(upstream: &Upstream, then: Then) -> Fixture {
        let kept = Arc::new(Kept::default());
        let model = RelayModel::new(label("priced"), label("priced"), vec![Wire::Chat])
            .unwrap()
            .with_pricing(Arc::new(Half));
        let relay = Relay::new(vec![model], Arc::new(Targets(upstream.authority.clone())))
            .unwrap()
            .with_translator(Arc::new(Scripted(then)))
            .with_records(Arc::clone(&kept) as Arc<dyn UsageRecords>);
        let config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        let verifier =
            Arc::new(SharedSecretVerifier::new(OwnerToken::new(OWNER.into()).unwrap()).unwrap());
        let handle = Gateway::bind_with_relay(
            config,
            verifier,
            RouteInventory::new(Vec::new()).unwrap(),
            relay,
        )
        .unwrap();
        handle.mark_ready();
        Fixture { handle, kept }
    }

    /// docs/gateway.md "The cost of a call": no cost after "terminal error after the head".
    #[test]
    fn adv_w03_a_translated_answer_ended_in_its_wires_terminal_error_has_no_cost() {
        let upstream = Upstream::start(Vec::new());
        let fixture = translating(&upstream, Then::Fails);
        let answered = post(
            fixture.address(),
            "/v1/messages",
            r#"{"model":"priced","stream":true}"#,
            Some(OWNER),
        );
        assert_eq!(answered.status, 200, "{answered:?}");
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].input_tokens, Some(12), "{:?}", records[0]);
        assert_eq!(records[0].cost_usd, None, "{:?}", records[0]);
    }

    /// docs/gateway.md "The cost of a call": no cost for a call with "a client that went away".
    #[test]
    fn adv_w03_a_call_whose_client_left_mid_answer_has_no_cost() {
        let upstream = Upstream::start(Vec::new());
        let fixture = translating(&upstream, Then::WaitsForTheClientToLeave);
        let body = r#"{"model":"priced","stream":true}"#;
        let mut client = TcpStream::connect(fixture.address()).unwrap();
        client.set_read_timeout(Some(WAIT)).unwrap();
        write!(
            client,
            "POST /v1/messages HTTP/1.1\r\nhost: g\r\n{}content-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            bearer(Some(OWNER)),
            body.len()
        )
        .unwrap();
        let mut raw = Vec::new();
        let mut piece = [0_u8; 4096];
        while !String::from_utf8_lossy(&raw).contains("message_start") {
            let read = client.read(&mut piece).unwrap();
            assert!(read > 0, "the answer ended early: {raw:?}");
            raw.extend_from_slice(&piece[..read]);
        }
        drop(client);
        let deadline = Instant::now() + WAIT;
        while fixture.records().is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let records = fixture.records();
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].input_tokens, Some(12), "{:?}", records[0]);
        assert_eq!(records[0].cost_usd, None, "{:?}", records[0]);
    }
}
