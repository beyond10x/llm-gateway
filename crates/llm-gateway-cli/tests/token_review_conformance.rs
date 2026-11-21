//! Conformance cases for the service-account review, written by an independent review of the
//! change: each drives `TokenReviewer` against a scripted `TokenReview` API over TLS on loopback
//! and checks one claim of `docs/gateway.md` ("Bounds of the service-account review"), of the
//! module documentation of `crates/llm-gateway-cli/src/token_review.rs`, or of the guide.
//! Nothing leaves the loopback interface.

mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use llm_gateway::{
    CallerReview, CallerToken, Gateway, GatewayConfig, GatewayHandle, IngressAuth, Label,
    OwnerToken, Principal, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, Review,
    RouteInventory, SharedSecretVerifier, TargetRefusal, Wire,
};
use llm_gateway_cli::{ReviewBounds, TokenReviewer, token_reviewer};
use rustls::{ServerConnection, StreamOwned};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::{Fixture, tls_pod::TestRoot};

const AUDIENCE: &str = "llm-gateway";
const AGENT: &str = "system:serviceaccount:agents:coding-agent";
const OWN_TOKEN: &str = "gateway-own-token-not-a-secret-0123456789";
const ROTATED_TOKEN: &str = "gateway-own-token-rotated-not-a-secret-9876543210";
const WAIT: Duration = Duration::from_secs(10);

fn later() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

/// A token shaped as a projected service-account token, distinct per `tag`.
fn jwt(tag: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"test"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        format!(
            r#"{{"aud":["{AUDIENCE}"],"exp":{},"sub":"{tag}"}}"#,
            later()
        )
        .as_bytes(),
    );
    let signature = URL_SAFE_NO_PAD.encode(format!("signature-of-{tag}").as_bytes());
    format!("{header}.{claims}.{signature}")
}

/// What the scripted API does with one review, once it has read it.
struct Script {
    /// Waited after the request is read and before the first answer byte.
    before_answer: Duration,
    /// The whole answer, head and body, written at once.
    answer: Vec<u8>,
}

fn http(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Answer\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn review_body(status: &serde_json::Value) -> String {
    serde_json::json!({
        "kind": "TokenReview",
        "apiVersion": "authentication.k8s.io/v1",
        "status": status,
    })
    .to_string()
}

fn now_answer(answer: Vec<u8>) -> Script {
    Script {
        before_answer: Duration::ZERO,
        answer,
    }
}

type Scripter = Arc<dyn Fn(&str) -> Script + Send + Sync>;

/// One review the scripted API read: the `authorization` it carried and its `spec.token`.
#[derive(Clone, Debug)]
struct Asked {
    authorization: Option<String>,
    token: String,
}

/// A `TokenReview` API over TLS on loopback for `localhost`, from a test root. It waits
/// `before_handshake` before it reads a connection's first byte, then answers as `script` says.
struct ScriptedApi {
    address: SocketAddr,
    root: TestRoot,
    asked: Arc<Mutex<Vec<Asked>>>,
}

impl ScriptedApi {
    fn start(before_handshake: Duration, script: Scripter) -> Self {
        let root = TestRoot::new("token review conformance root");
        let config = root.server("localhost");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
        let recording = Arc::clone(&asked);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(connection) = connection else { return };
                let (config, recording, script) = (
                    Arc::clone(&config),
                    Arc::clone(&recording),
                    Arc::clone(&script),
                );
                thread::spawn(move || {
                    thread::sleep(before_handshake);
                    connection.set_read_timeout(Some(WAIT)).unwrap();
                    let mut stream =
                        StreamOwned::new(ServerConnection::new(config).unwrap(), connection);
                    let Some(asked) = read_review(&mut stream) else {
                        return;
                    };
                    let scripted = script(&asked.token);
                    recording.lock().unwrap().push(asked);
                    thread::sleep(scripted.before_answer);
                    drop(stream.write_all(&scripted.answer));
                    drop(stream.flush());
                    stream.conn.send_close_notify();
                    drop(stream.flush());
                });
            }
        });
        Self {
            address,
            root,
            asked,
        }
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }

    fn ca_pem(&self) -> String {
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.root.der);
        let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
        for line in encoded.as_bytes().chunks(64) {
            pem.push_str(std::str::from_utf8(line).unwrap());
            pem.push('\n');
        }
        pem.push_str("-----END CERTIFICATE-----\n");
        pem
    }
}

fn read_review(stream: &mut impl Read) -> Option<Asked> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    let end = loop {
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&raw[..end]).into_owned();
    let field = |name: &str| {
        head.split("\r\n").find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    let length: usize = field("content-length")?.parse().ok()?;
    while raw.len() < end + 4 + length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..read]);
    }
    let body: serde_json::Value = serde_json::from_slice(&raw[end + 4..end + 4 + length]).ok()?;
    Some(Asked {
        authorization: field("authorization"),
        token: body["spec"]["token"].as_str().unwrap_or("").to_owned(),
    })
}

/// The CA and the gateway's own token as a kubelet mounts them: `token` and `ca.crt` are links
/// into `..data`, itself a link to a timestamped directory that a rotation replaces atomically.
struct Mounted {
    fixture: Fixture,
    ca: PathBuf,
    own: PathBuf,
}

impl Mounted {
    fn new(name: &str, api: &ScriptedApi) -> Self {
        let fixture = Fixture::new(name);
        let mounted = Self {
            ca: fixture.path("ca.crt"),
            own: fixture.path("token"),
            fixture,
        };
        mounted.publish("..2026_11_21_01", &api.ca_pem(), OWN_TOKEN);
        std::os::unix::fs::symlink("..data/ca.crt", &mounted.ca).unwrap();
        std::os::unix::fs::symlink("..data/token", &mounted.own).unwrap();
        mounted
    }

    /// Writes a new timestamped directory and swings `..data` to it with one rename.
    fn publish(&self, stamp: &str, ca: &str, own: &str) {
        let directory = self.fixture.path(stamp);
        std::fs::create_dir_all(&directory).unwrap();
        for (name, bytes) in [("ca.crt", ca), ("token", own)] {
            let file = directory.join(name);
            std::fs::write(&file, bytes).unwrap();
            std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644))
                .unwrap();
        }
        let staged = self.fixture.path("..data_tmp");
        drop(std::fs::remove_file(&staged));
        std::os::unix::fs::symlink(stamp, &staged).unwrap();
        std::fs::rename(&staged, self.fixture.path("..data")).unwrap();
    }
}

fn reviewer_at(api: &ScriptedApi, mounted: &Mounted, principals: &[&str]) -> TokenReviewer {
    reviewer_for(
        &format!("https://localhost:{}", api.address.port()),
        mounted,
        principals,
    )
}

fn reviewer_for(api: &str, mounted: &Mounted, principals: &[&str]) -> TokenReviewer {
    let principals: Vec<String> = principals.iter().map(|p| format!("\"{p}\"")).collect();
    let text = format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"service-account-token\"\n\n[hosted.open]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:1\"\nauth_kind = \"anonymous\"\n\n[token_review]\naudience = \"{AUDIENCE}\"\nprincipals = [{}]\napi = \"{api}\"\nca_file = \"{}\"\ncredential_file = \"{}\"\n",
        principals.join(", "),
        mounted.ca.display(),
        mounted.own.display(),
    );
    token_reviewer(&llm_gateway_cli::parse(&text).unwrap())
        .unwrap()
        .expect("a service-account-token document composes a reviewer")
}

fn token(text: &str) -> CallerToken {
    CallerToken::new(text.as_bytes().to_vec()).unwrap()
}

fn admitted(principal: &str) -> Review {
    Review::Admitted(Principal::new(principal).unwrap())
}

// --- Fail closed: `status.error` -----------------------------------------------------------------

/// A `TokenReview` whose `status.error` is set says the authenticator could not decide; an
/// answer carrying it is not an admission, whatever `authenticated` says (item 1 of the review's
/// brief: "`status.error` set ... refuses").
#[test]
fn conformance_an_answer_with_status_error_set_is_never_an_admission() {
    let script: Scripter = Arc::new(|_| {
        now_answer(http(
            201,
            &review_body(&serde_json::json!({
                "authenticated": true,
                "user": {"username": AGENT},
                "audiences": [AUDIENCE],
                "error": "an authenticator failed to verify the token",
            })),
        ))
    });
    let api = ScriptedApi::start(Duration::ZERO, script);
    let mounted = Mounted::new("conformance-status-error", &api);
    let reviewer = reviewer_at(&api, &mounted, &[AGENT]);
    let decided = reviewer.review(&token(&jwt("status-error")));
    assert_ne!(
        decided,
        admitted(AGENT),
        "a review answer with status.error set admitted the caller"
    );
}

// --- The published timeout -----------------------------------------------------------------------

/// `docs/gateway.md`: "One review, connection, handshake, request and answer, ends within
/// `review-timeout-ms` or is `credential-review-unavailable`." A slow API server that spends
/// most of the bound on the handshake and most of it again before answering must not hold the
/// review (and one of its `concurrent-reviews` slots) for about twice the bound.
#[test]
fn conformance_one_review_ends_within_the_published_timeout() {
    let bound = Duration::from_millis(1000);
    let script: Scripter = Arc::new(|_| Script {
        before_answer: Duration::from_millis(850),
        answer: http(
            201,
            &review_body(&serde_json::json!({
                "authenticated": true,
                "user": {"username": AGENT},
                "audiences": [AUDIENCE],
            })),
        ),
    });
    let api = ScriptedApi::start(Duration::from_millis(850), script);
    let mounted = Mounted::new("conformance-timeout", &api);
    let reviewer = reviewer_at(&api, &mounted, &[AGENT]).with_bounds(ReviewBounds {
        timeout: bound,
        ..ReviewBounds::default()
    });
    let started = Instant::now();
    let decided = reviewer.review(&token(&jwt("slow")));
    let took = started.elapsed();
    assert_eq!(decided, Review::Unavailable, "a late answer was used");
    assert!(
        took < bound + Duration::from_millis(250),
        "one review took {took:?} under a published bound of {bound:?}"
    );
}

// --- A flood of forged tokens naming the audience: the documented bound -------------------------

const OWNER: &str = "owner-token-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CHAT_BODY: &str = r#"{"model":"open","messages":[]}"#;

/// A loopback model answering every request `200 {}`.
fn model_pod() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let authority = listener.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !raw.windows(4).any(|window| window == b"\r\n\r\n")
                || raw.len()
                    < raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4 + CHAT_BODY.len()
            {
                match connection.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&chunk[..read]),
                }
            }
            drop(connection.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
            ));
        }
    });
    authority
}

struct PodSource(String);

struct PodTarget(String);

impl RelayTarget for PodTarget {
    fn authority(&self) -> &str {
        &self.0
    }

    fn connect(&self) -> std::io::Result<Box<dyn RelayStream>> {
        let stream = TcpStream::connect(&self.0)?;
        stream.set_read_timeout(Some(WAIT))?;
        stream.set_write_timeout(Some(WAIT))?;
        Ok(Box::new(stream))
    }
}

impl RelayTargets for PodSource {
    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Ok(Box::new(PodTarget(self.0.clone())))
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// The reviewer as the gateway is composed with it, counting every time it is shown the owner's
/// bearer.
#[derive(Debug)]
struct Watched {
    reviewer: Arc<TokenReviewer>,
    shown_the_owner: AtomicUsize,
}

impl CallerReview for Watched {
    fn review(&self, presented: &CallerToken) -> Review {
        if presented.expose() == OWNER.as_bytes() {
            self.shown_the_owner.fetch_add(1, Ordering::SeqCst);
        }
        self.reviewer.review(presented)
    }
}

/// A gateway under `service-account-token` with the owner secret, its relay composed with
/// `review` and one model on a loopback pod.
fn reviewed_gateway(review: Arc<Watched>) -> GatewayHandle {
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = WAIT;
    config.ingress_auth = IngressAuth::ServiceAccountToken;
    let model = RelayModel::new(
        Label::new("open").unwrap(),
        Label::new("served-open").unwrap(),
        vec![Wire::Chat],
    )
    .unwrap();
    let relay = Relay::new(vec![model], Arc::new(PodSource(model_pod())))
        .unwrap()
        .with_caller_review(review as Arc<dyn CallerReview>);
    let owner = Arc::new(
        SharedSecretVerifier::new(OwnerToken::new(OWNER.as_bytes().to_vec()).unwrap()).unwrap(),
    );
    let handle = Gateway::bind_with_relay(
        config,
        owner,
        RouteInventory::new(Vec::new()).unwrap(),
        relay,
    )
    .unwrap();
    handle.mark_ready();
    handle
}

/// `(status, code)` of one chat call to `address` presenting `bearer`; `relayed` for a 200.
fn chat(address: SocketAddr, bearer: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {bearer}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{CHAT_BODY}",
        CHAT_BODY.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
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

/// The behaviour the security review found (a caller with no valid token who knows the audience
/// can fill `concurrent-reviews`) is accepted behind the guide's `NetworkPolicy`, and this pins its
/// documented bound. The review's scenario: twelve workers send distinct JWT-shaped tokens naming
/// the audience, which the API server holds and then rejects. Meanwhile (a) no more than 8
/// reviews are ever in flight at the API server; (b) a listed caller whose admission is cached is
/// relayed without a review; (c) a listed caller with no cached admission is `503
/// credential-review-unavailable`, never relayed and never `credential-rejected`, and is relayed
/// as soon as a slot is free; (d) the owner's bearer is relayed throughout and never reviewed.
#[test]
#[allow(clippy::too_many_lines)]
fn conformance_a_flood_naming_the_audience_holds_eight_reviews_and_refuses_only_uncached_callers_with_503()
 {
    let cached = jwt("cached");
    let listed = jwt("listed");
    let in_flight = Arc::new(AtomicUsize::new(0));
    let most_in_flight = Arc::new(AtomicUsize::new(0));
    let (admitting, counting, most) = (
        [cached.clone(), listed.clone()],
        Arc::clone(&in_flight),
        Arc::clone(&most_in_flight),
    );
    let script: Scripter = Arc::new(move |presented| {
        if admitting.iter().any(|token| token == presented) {
            return now_answer(http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": AGENT},
                    "audiences": [AUDIENCE],
                })),
            ));
        }
        // A forged token: held at the API server, then rejected.
        let now = counting.fetch_add(1, Ordering::SeqCst) + 1;
        most.fetch_max(now, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(1500));
        counting.fetch_sub(1, Ordering::SeqCst);
        now_answer(http(
            201,
            &review_body(&serde_json::json!({"authenticated": false, "error": "invalid"})),
        ))
    });
    let api = ScriptedApi::start(Duration::ZERO, script);
    let mounted = Mounted::new("conformance-flood", &api);
    let reviewer = Arc::new(reviewer_at(&api, &mounted, &[AGENT]));
    let watched = Arc::new(Watched {
        reviewer: Arc::clone(&reviewer),
        shown_the_owner: AtomicUsize::new(0),
    });
    let gateway = reviewed_gateway(Arc::clone(&watched));
    let address = gateway.local_addr();
    assert_eq!(chat(address, &cached), (200, "relayed".to_owned()));

    let flooding = Arc::new(AtomicBool::new(true));
    let floods: Vec<_> = (0..12)
        .map(|worker| {
            let (reviewer, flooding) = (Arc::clone(&reviewer), Arc::clone(&flooding));
            thread::spawn(move || {
                let mut n = 0_u64;
                while flooding.load(Ordering::SeqCst) {
                    let decided = reviewer.review(&token(&jwt(&format!("flood-{worker}-{n}"))));
                    if decided == Review::Unavailable {
                        thread::sleep(Duration::from_millis(1));
                    }
                    n += 1;
                }
            })
        })
        .collect();
    let deadline = Instant::now() + WAIT;
    while in_flight.load(Ordering::SeqCst) < 8 {
        assert!(
            Instant::now() < deadline,
            "the flood never filled the review slots"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // (b) and (d) while every slot is taken.
    assert_eq!(
        chat(address, &cached),
        (200, "relayed".to_owned()),
        "(b) cached caller"
    );
    assert_eq!(
        chat(address, OWNER),
        (200, "relayed".to_owned()),
        "(d) owner"
    );
    // (c) an uncached listed caller while every slot is taken.
    assert_eq!(
        chat(address, &listed),
        (503, "credential-review-unavailable".to_owned()),
        "(c) uncached caller during the flood"
    );

    flooding.store(false, Ordering::SeqCst);
    // (c) relayed as soon as a slot is free; until then only ever 503.
    let deadline = Instant::now() + WAIT;
    loop {
        match chat(address, &listed) {
            (200, _) => break,
            (503, code) if code == "credential-review-unavailable" => {
                assert!(Instant::now() < deadline, "(c) no slot came free");
                thread::sleep(Duration::from_millis(20));
            }
            other => panic!("(c) an uncached listed caller was answered {other:?}"),
        }
    }
    assert_eq!(
        chat(address, OWNER),
        (200, "relayed".to_owned()),
        "(d) owner after"
    );
    for flood in floods {
        flood.join().unwrap();
    }

    // (a) never more than 8 at the API server, and the flood did reach 8.
    assert_eq!(
        most_in_flight.load(Ordering::SeqCst),
        8,
        "(a) reviews in flight"
    );
    let asked = api.asked();
    assert_eq!(
        asked.iter().filter(|asked| asked.token == cached).count(),
        1,
        "(b) the cached caller was reviewed again"
    );
    assert_eq!(
        watched.shown_the_owner.load(Ordering::SeqCst),
        0,
        "(d) the owner's bearer was shown to the review"
    );
    drop(gateway);
}

// --- What the change enforces (probes that hold) ------------------------------------------------

/// Every answer below is one an API server, a proxy in front of it or an old API server could
/// give; none of them may admit, and none that is not a review may be kept.
#[test]
#[allow(clippy::too_many_lines)]
fn conformance_no_odd_answer_admits() {
    let huge = format!(
        r#"{{"kind":"TokenReview","status":{{"authenticated":true,"user":{{"username":"{AGENT}"}},"audiences":["{AUDIENCE}"]}},"pad":"{}"}}"#,
        "x".repeat(70 * 1024)
    );
    let answers: Vec<(&str, Vec<u8>)> = vec![
        (
            "no audiences field (an API server that ignores spec.audiences)",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true, "user": {"username": AGENT}})),
            ),
        ),
        (
            "audiences null",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true, "user": {"username": AGENT}, "audiences": null})),
            ),
        ),
        (
            "authenticated missing",
            http(
                201,
                &review_body(&serde_json::json!({
                    "user": {"username": AGENT}, "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "authenticated as a string",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": "true", "user": {"username": AGENT}, "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "authenticated as a number",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": 1, "user": {"username": AGENT}, "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "username missing",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true, "user": {}, "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "username upper case",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": "system:serviceaccount:agents:Coding-agent"},
                    "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "username with a trailing space",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": format!("{AGENT} ")},
                    "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "username a longer name with the listed one as prefix",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": format!("{AGENT}-2")},
                    "audiences": [AUDIENCE]})),
            ),
        ),
        (
            "audience upper case",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": AGENT},
                    "audiences": ["LLM-GATEWAY"]})),
            ),
        ),
        (
            "audience with the configured one as prefix",
            http(
                201,
                &review_body(&serde_json::json!({
                    "authenticated": true,
                    "user": {"username": AGENT},
                    "audiences": [format!("{AUDIENCE}-x")]})),
            ),
        ),
        (
            "truncated body",
            {
                let mut answer = http(
                    201,
                    &review_body(&serde_json::json!({
                        "authenticated": true, "user": {"username": AGENT}, "audiences": [AUDIENCE]})),
                );
                answer.truncate(answer.len() - 3);
                answer
            },
        ),
        ("an answer past 64 KiB", http(201, &huge)),
        (
            "a redirect",
            b"HTTP/1.1 302 Found\r\nlocation: https://localhost:1/apis/authentication.k8s.io/v1/tokenreviews\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
        ),
        ("a 204", http(204, "")),
        (
            "a Status object with 201",
            http(201, r#"{"kind":"Status","status":"Failure"}"#),
        ),
        ("no head end", b"HTTP/1.1 201 Answer\r\n".to_vec()),
    ];
    let scripted: Arc<Mutex<Vec<u8>>> = Arc::default();
    let reading = Arc::clone(&scripted);
    let script: Scripter = Arc::new(move |_| now_answer(reading.lock().unwrap().clone()));
    let api = ScriptedApi::start(Duration::ZERO, script);
    let mounted = Mounted::new("conformance-odd-answers", &api);
    let reviewer = reviewer_at(&api, &mounted, &[AGENT]).with_bounds(ReviewBounds {
        timeout: Duration::from_secs(2),
        ..ReviewBounds::default()
    });
    for (n, (case, answer)) in answers.into_iter().enumerate() {
        *scripted.lock().unwrap() = answer;
        let decided = reviewer.review(&token(&jwt(&format!("odd-{n}"))));
        assert!(
            matches!(decided, Review::Rejected | Review::Unavailable),
            "{case}: {decided:?}"
        );
    }
    assert_eq!(
        api.asked().len(),
        17,
        "a redirect was followed or a review was skipped"
    );
}

/// `token_review.credential_file` is read at every review: once the kubelet swings `..data` to
/// a rotated token, the next review presents the rotated one, through the trusted reader.
#[test]
fn conformance_a_rotated_own_token_is_presented_from_the_next_review_on() {
    let script: Scripter = Arc::new(|_| {
        now_answer(http(
            201,
            &review_body(&serde_json::json!({
                "authenticated": true,
                "user": {"username": AGENT},
                "audiences": [AUDIENCE],
            })),
        ))
    });
    let api = ScriptedApi::start(Duration::ZERO, script);
    let mounted = Mounted::new("conformance-rotation", &api);
    let reviewer = reviewer_at(&api, &mounted, &[AGENT]);
    assert_eq!(reviewer.review(&token(&jwt("before"))), admitted(AGENT));
    mounted.publish("..2026_11_21_02", &api.ca_pem(), ROTATED_TOKEN);
    assert_eq!(reviewer.review(&token(&jwt("after"))), admitted(AGENT));
    let presented: Vec<Option<String>> = api
        .asked()
        .into_iter()
        .map(|asked| asked.authorization)
        .collect();
    assert_eq!(
        presented,
        vec![
            Some(format!("Bearer {OWN_TOKEN}")),
            Some(format!("Bearer {ROTATED_TOKEN}"))
        ]
    );
}

/// The API's certificate is verified for the host the `api` URL names: the same server, trusted
/// CA and all, reached as `127.0.0.1` (a name its certificate does not hold) sends no review.
#[test]
fn conformance_the_api_host_name_is_verified() {
    let script: Scripter = Arc::new(|_| {
        now_answer(http(
            201,
            &review_body(&serde_json::json!({
                "authenticated": true,
                "user": {"username": AGENT},
                "audiences": [AUDIENCE],
            })),
        ))
    });
    let api = ScriptedApi::start(Duration::ZERO, script);
    let mounted = Mounted::new("conformance-host-name", &api);
    let reviewer = reviewer_for(
        &format!("https://127.0.0.1:{}", api.address.port()),
        &mounted,
        &[AGENT],
    );
    assert_eq!(
        reviewer.review(&token(&jwt("by-address"))),
        Review::Unavailable
    );
    assert!(
        api.asked().is_empty(),
        "a review reached a host its certificate does not name"
    );
}
