//! The binary's service-account ingress (story:gateway-sa-token-ingress): the `[token_review]`
//! keys of the closed document, the `TokenReview` call against a fake `TokenReview` API served over
//! TLS on loopback with a test root, every way it fails closed, the review cache's bounds, and the
//! shipped binary admitting a reviewed token, recording its principal and logging no token.
//! Nothing leaves the loopback interface and no cluster is reached.

mod support;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use llm_gateway::{CallerReview, CallerToken, Principal, Review};
use llm_gateway_cli::{Clock, ReviewBounds, TOKEN_REVIEW_PATH, TokenReviewer, token_reviewer};
use rustls::{ServerConnection, StreamOwned};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::{Fixture, OWNER_SECRET, tls_pod::TestRoot};

const AUDIENCE: &str = "llm-gateway";
const AGENT: &str = "system:serviceaccount:agents:coding-agent";
const OTHER_AGENT: &str = "system:serviceaccount:agents:other-agent";
/// The gateway's own service-account token, which it presents to the `TokenReview` API.
const OWN_TOKEN: &str = "gateway-own-token-not-a-secret-0123456789";
const WAIT: Duration = Duration::from_secs(10);
const LISTENING: &str = "b10x-llm-gateway: listening on ";

/// Seconds since the Unix epoch, now.
fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A token shaped as a projected service-account token: three base64url segments, the middle
/// one holding `exp` (when given) and a `sub` naming `tag`.
fn jwt(tag: &str, exp: Option<u64>) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"test"}"#);
    let claims = match exp {
        Some(exp) => format!(r#"{{"aud":["{AUDIENCE}"],"exp":{exp},"sub":"{tag}"}}"#),
        None => format!(r#"{{"aud":["{AUDIENCE}"],"sub":"{tag}"}}"#),
    };
    let claims = URL_SAFE_NO_PAD.encode(claims.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(format!("signature-of-{tag}").as_bytes());
    format!("{header}.{claims}.{signature}")
}

/// A token shaped as a JWT whose claims are exactly `claims`.
fn jwt_claiming(claims: &str) -> String {
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","kid":"test"}"#);
    let signature = URL_SAFE_NO_PAD.encode(b"forged-signature");
    format!(
        "{header}.{}.{signature}",
        URL_SAFE_NO_PAD.encode(claims.as_bytes())
    )
}

fn later() -> u64 {
    now_seconds() + 3600
}

/// What the fake `TokenReview` API answers for one token.
#[derive(Clone)]
enum Answer {
    /// A `201` `TokenReview`.
    Review {
        authenticated: bool,
        username: &'static str,
        audiences: Vec<&'static str>,
    },
    /// This status with an empty JSON object.
    Status(u16),
    /// A `201` whose body is not a `TokenReview`.
    Garbage,
    /// A `201` admission for this user and the audience that also sets `status.error`.
    Erred(&'static str),
    /// Nothing, for this long.
    Hang(Duration),
}

fn admits(username: &'static str) -> Answer {
    Answer::Review {
        authenticated: true,
        username,
        audiences: vec![AUDIENCE],
    }
}

/// One request the fake API decrypted.
#[derive(Clone, Debug)]
struct Asked {
    request_line: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

/// A fake `TokenReview` API over TLS on loopback, with a certificate for `localhost` from a test
/// root. It answers each review as `answers` says for its `spec.token`, and records each.
struct FakeApi {
    address: SocketAddr,
    answers: Arc<Mutex<BTreeMap<String, Answer>>>,
    asked: Arc<Mutex<Vec<Asked>>>,
    root: TestRoot,
}

impl FakeApi {
    fn start() -> Self {
        let root = TestRoot::new("token review test root");
        let config = root.server("localhost");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let answers: Arc<Mutex<BTreeMap<String, Answer>>> = Arc::default();
        let asked: Arc<Mutex<Vec<Asked>>> = Arc::default();
        let (scripted, recording) = (Arc::clone(&answers), Arc::clone(&asked));
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(connection) = connection else { return };
                let config = Arc::clone(&config);
                let (scripted, recording) = (Arc::clone(&scripted), Arc::clone(&recording));
                thread::spawn(move || {
                    connection.set_read_timeout(Some(WAIT)).unwrap();
                    let tls = ServerConnection::new(config).unwrap();
                    let mut stream = StreamOwned::new(tls, connection);
                    let Some(asked) = read_review(&mut stream) else {
                        return;
                    };
                    let token = asked.body["spec"]["token"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned();
                    recording.lock().unwrap().push(asked);
                    let answer =
                        scripted
                            .lock()
                            .unwrap()
                            .get(&token)
                            .cloned()
                            .unwrap_or(Answer::Review {
                                authenticated: false,
                                username: "",
                                audiences: vec![],
                            });
                    let (status, body) = match answer {
                        Answer::Review {
                            authenticated,
                            username,
                            audiences,
                        } => {
                            let mut status = serde_json::json!({"authenticated": authenticated});
                            if authenticated {
                                status["user"] = serde_json::json!({"username": username});
                                status["audiences"] = serde_json::json!(audiences);
                            } else {
                                status["error"] = serde_json::json!("token is invalid");
                            }
                            let review = serde_json::json!({
                                "kind": "TokenReview",
                                "apiVersion": "authentication.k8s.io/v1",
                                "spec": {"token": token},
                                "status": status,
                            });
                            (201, review.to_string())
                        }
                        Answer::Status(status) => (status, "{}".to_owned()),
                        Answer::Garbage => (201, "<html>not a review</html>".to_owned()),
                        Answer::Erred(username) => {
                            let review = serde_json::json!({
                                "kind": "TokenReview",
                                "status": {
                                    "authenticated": true,
                                    "user": {"username": username},
                                    "audiences": [AUDIENCE],
                                    "error": "an authenticator failed",
                                },
                            });
                            (201, review.to_string())
                        }
                        Answer::Hang(time) => {
                            thread::sleep(time);
                            return;
                        }
                    };
                    let answer = format!(
                        "HTTP/1.1 {status} Answer\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    drop(stream.write_all(answer.as_bytes()));
                    drop(stream.flush());
                    stream.conn.send_close_notify();
                    drop(stream.flush());
                });
            }
        });
        Self {
            address,
            answers,
            asked,
            root,
        }
    }

    fn answer(&self, token: &str, answer: Answer) {
        self.answers
            .lock()
            .unwrap()
            .insert(token.to_owned(), answer);
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }

    fn count(&self) -> usize {
        self.asked.lock().unwrap().len()
    }

    fn api(&self) -> String {
        format!("https://localhost:{}", self.address.port())
    }

    /// The test root as the PEM `ca_file` holds.
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
    Some(Asked {
        request_line: head.split("\r\n").next().unwrap_or("").to_owned(),
        authorization: field("authorization"),
        body: serde_json::from_slice(&raw[end + 4..end + 4 + length]).ok()?,
    })
}

/// The `[token_review]` table for `api`, the CA at `ca` and the gateway's token at `own`.
fn token_review_table(api: &str, ca: &Path, own: &Path, principals: &[&str]) -> String {
    let principals: Vec<String> = principals.iter().map(|p| format!("\"{p}\"")).collect();
    format!(
        "[token_review]\naudience = \"{AUDIENCE}\"\nprincipals = [{}]\napi = \"{api}\"\nca_file = \"{}\"\ncredential_file = \"{}\"\n",
        principals.join(", "),
        ca.display(),
        own.display()
    )
}

/// A document under `service-account-token` serving one anonymous hosted model at `base_url`.
fn document(listen: &str, owner: Option<&Path>, base_url: &str, table: &str) -> String {
    let owner = owner.map_or(String::new(), |path| {
        format!("owner_secret_file = \"{}\"\n", path.display())
    });
    format!(
        "listen = \"{listen}\"\ningress_auth = \"service-account-token\"\n{owner}\n[hosted.open]\nwires = [\"chat\"]\nbase_url = \"{base_url}\"\nauth_kind = \"anonymous\"\n\n{table}"
    )
}

/// The CA and the gateway's own token in `fixture`, as a kubelet presents them: each a regular
/// file behind a symbolic link into a `..data` directory, the token mode 0644.
fn mounted(fixture: &Fixture, api: &FakeApi) -> (PathBuf, PathBuf) {
    let data = fixture.path("..data");
    std::fs::create_dir_all(&data).unwrap();
    let write = |name: &str, bytes: &[u8]| {
        let file = data.join(name);
        std::fs::write(&file, bytes).unwrap();
        std::fs::set_permissions(&file, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
        let link = fixture.path(name);
        drop(std::fs::remove_file(&link));
        std::os::unix::fs::symlink(format!("..data/{name}"), &link).unwrap();
        link
    };
    let ca = write("ca.crt", api.ca_pem().as_bytes());
    let own = write("token", OWN_TOKEN.as_bytes());
    (ca, own)
}

/// A reviewer over `api` admitting `principals`, on a clock the test moves.
fn reviewer(name: &str, api: &FakeApi, principals: &[&str]) -> (TokenReviewer, Arc<AtomicU64>) {
    let fixture = Fixture::new(name);
    let (ca, own) = mounted(&fixture, api);
    let text = document(
        "127.0.0.1:0",
        None,
        "http://127.0.0.1:1",
        &token_review_table(&api.api(), &ca, &own, principals),
    );
    let deployment = llm_gateway_cli::parse(&text).unwrap();
    let now = Arc::new(AtomicU64::new(now_seconds() * 1000));
    let reading = Arc::clone(&now);
    let clock: Clock = Arc::new(move || reading.load(Ordering::SeqCst));
    let reviewer = token_reviewer(&deployment)
        .unwrap()
        .expect("a service-account-token document composes a reviewer")
        .with_clock(clock);
    (reviewer, now)
}

fn token(text: &str) -> CallerToken {
    CallerToken::new(text.as_bytes().to_vec()).unwrap()
}

fn admitted(principal: &str) -> Review {
    Review::Admitted(Principal::new(principal).unwrap())
}

fn advance(clock: &AtomicU64, by: Duration) {
    clock.fetch_add(u64::try_from(by.as_millis()).unwrap(), Ordering::SeqCst);
}

// --- The review ----------------------------------------------------------------------------------

#[test]
fn sa_an_allow_listed_account_for_the_audience_is_admitted_by_one_review() {
    let api = FakeApi::start();
    let agent = jwt("agent", Some(later()));
    api.answer(&agent, admits(AGENT));
    let (reviewer, _) = reviewer("sa-admitted", &api, &[AGENT]);
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    let asked = api.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].request_line,
        format!("POST {TOKEN_REVIEW_PATH} HTTP/1.1")
    );
    assert_eq!(
        asked[0].authorization.as_deref(),
        Some(format!("Bearer {OWN_TOKEN}").as_str()),
        "the review is made with the gateway's own token"
    );
    assert_eq!(asked[0].body["kind"], "TokenReview");
    assert_eq!(asked[0].body["apiVersion"], "authentication.k8s.io/v1");
    assert_eq!(asked[0].body["spec"]["token"], agent.as_str());
    assert_eq!(
        asked[0].body["spec"]["audiences"],
        serde_json::json!([AUDIENCE])
    );
}

#[test]
fn sa_every_failure_of_the_review_refuses_and_none_admits() {
    let api = FakeApi::start();
    let (reviewer, _) = reviewer("sa-failures", &api, &[AGENT]);
    let reviewer = reviewer.with_bounds(ReviewBounds {
        timeout: Duration::from_millis(500),
        ..ReviewBounds::default()
    });
    let cases: Vec<(&str, Option<Answer>, Review, usize)> = vec![
        (
            "another audience",
            Some(Answer::Review {
                authenticated: true,
                username: AGENT,
                audiences: vec!["another-audience"],
            }),
            Review::Rejected,
            1,
        ),
        (
            "no audience at all",
            Some(Answer::Review {
                authenticated: true,
                username: AGENT,
                audiences: vec![],
            }),
            Review::Rejected,
            1,
        ),
        (
            "a principal off the list",
            Some(admits(OTHER_AGENT)),
            Review::Rejected,
            1,
        ),
        (
            "not a service account",
            Some(admits("system:node:worker-1")),
            Review::Rejected,
            1,
        ),
        ("unauthenticated", None, Review::Rejected, 1),
        (
            "a non-success answer",
            Some(Answer::Status(500)),
            Review::Unavailable,
            1,
        ),
        (
            "an unauthorized gateway",
            Some(Answer::Status(403)),
            Review::Unavailable,
            1,
        ),
        (
            "a malformed answer",
            Some(Answer::Garbage),
            Review::Unavailable,
            1,
        ),
        (
            "a timeout",
            Some(Answer::Hang(Duration::from_secs(3))),
            Review::Unavailable,
            1,
        ),
    ];
    for (case, answer, expected, reviews) in cases {
        let presented = jwt(case, Some(later()));
        if let Some(answer) = answer {
            api.answer(&presented, answer);
        }
        let before = api.count();
        let started = Instant::now();
        assert_eq!(reviewer.review(&token(&presented)), expected, "{case}");
        assert_eq!(api.count() - before, reviews, "{case}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{case} outlived its timeout"
        );
    }
    // An expired token and a bearer that is no service-account token cost no review.
    let before = api.count();
    let expired = jwt("expired", Some(now_seconds() - 1));
    api.answer(&expired, admits(AGENT));
    assert_eq!(reviewer.review(&token(&expired)), Review::Rejected);
    assert_eq!(reviewer.review(&token("not-a-jwt")), Review::Rejected);
    assert_eq!(reviewer.review(&token("a.b.c")), Review::Rejected);
    assert_eq!(
        api.count(),
        before,
        "a token refused on its face was reviewed"
    );
}

#[test]
fn sa_an_unreachable_api_is_unavailable() {
    let api = FakeApi::start();
    let fixture = Fixture::new("sa-unreachable");
    let (ca, own) = mounted(&fixture, &api);
    // A port nothing listens on, held by the test so no other test can bind it.
    let held = support::Refusing::new();
    let closed = held.address.port();
    let text = document(
        "127.0.0.1:0",
        None,
        "http://127.0.0.1:1",
        &token_review_table(&format!("https://localhost:{closed}"), &ca, &own, &[AGENT]),
    );
    let reviewer = token_reviewer(&llm_gateway_cli::parse(&text).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        reviewer.review(&token(&jwt("agent", Some(later())))),
        Review::Unavailable
    );
}

#[test]
fn sa_a_certificate_the_ca_did_not_issue_is_unavailable_and_sends_no_token() {
    let api = FakeApi::start();
    let other = FakeApi::start();
    let fixture = Fixture::new("sa-other-ca");
    // The CA of `other`, the API of `api`.
    let (ca, own) = mounted(&fixture, &other);
    let text = document(
        "127.0.0.1:0",
        None,
        "http://127.0.0.1:1",
        &token_review_table(&api.api(), &ca, &own, &[AGENT]),
    );
    let reviewer = token_reviewer(&llm_gateway_cli::parse(&text).unwrap())
        .unwrap()
        .unwrap();
    let agent = jwt("agent", Some(later()));
    api.answer(&agent, admits(AGENT));
    assert_eq!(reviewer.review(&token(&agent)), Review::Unavailable);
    assert_eq!(
        api.count(),
        0,
        "a request reached an API the CA did not vouch for"
    );
}

// --- The cache -----------------------------------------------------------------------------------

#[test]
fn sa_an_admission_is_kept_sixty_seconds_and_then_reviewed_again() {
    let api = FakeApi::start();
    let agent = jwt("agent", Some(later()));
    api.answer(&agent, admits(AGENT));
    let (reviewer, clock) = reviewer("sa-cache-window", &api, &[AGENT]);
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    advance(&clock, Duration::from_secs(59));
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    assert_eq!(
        api.count(),
        1,
        "a second call within the window was reviewed"
    );
    advance(&clock, Duration::from_secs(2));
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    assert_eq!(api.count(), 2, "a call after the window was not reviewed");
}

#[test]
fn sa_an_admission_never_outlives_the_tokens_own_expiry() {
    let api = FakeApi::start();
    let agent = jwt("agent", Some(now_seconds() + 10));
    api.answer(&agent, admits(AGENT));
    let (reviewer, clock) = reviewer("sa-cache-expiry", &api, &[AGENT]);
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    advance(&clock, Duration::from_secs(5));
    assert_eq!(reviewer.review(&token(&agent)), admitted(AGENT));
    advance(&clock, Duration::from_secs(6));
    assert_eq!(
        reviewer.review(&token(&agent)),
        Review::Rejected,
        "an expired token was admitted from the cache"
    );
    assert_eq!(api.count(), 1);
}

#[test]
fn sa_a_rejection_is_kept_three_seconds_at_most_and_an_unavailable_answer_not_at_all() {
    let api = FakeApi::start();
    let refused = jwt("refused", Some(later()));
    let failing = jwt("failing", Some(later()));
    api.answer(&refused, admits(OTHER_AGENT));
    api.answer(&failing, Answer::Status(503));
    let (reviewer, clock) = reviewer("sa-cache-rejection", &api, &[AGENT]);
    assert_eq!(reviewer.review(&token(&refused)), Review::Rejected);
    advance(&clock, Duration::from_secs(2));
    assert_eq!(reviewer.review(&token(&refused)), Review::Rejected);
    assert_eq!(api.count(), 1);
    advance(&clock, Duration::from_secs(2));
    assert_eq!(reviewer.review(&token(&refused)), Review::Rejected);
    assert_eq!(api.count(), 2, "a rejection was kept past three seconds");
    assert_eq!(reviewer.review(&token(&failing)), Review::Unavailable);
    assert_eq!(reviewer.review(&token(&failing)), Review::Unavailable);
    assert_eq!(api.count(), 4, "an unavailable answer was kept");
}

#[test]
fn sa_the_cache_holds_its_entry_bound_and_lets_the_soonest_to_end_go_first() {
    let api = FakeApi::start();
    let tokens: Vec<String> = (0..3)
        .map(|n| jwt(&format!("agent-{n}"), Some(later())))
        .collect();
    for presented in &tokens {
        api.answer(presented, admits(AGENT));
    }
    let (reviewer, clock) = reviewer("sa-cache-entries", &api, &[AGENT]);
    let reviewer = reviewer.with_bounds(ReviewBounds {
        entries: 2,
        ..ReviewBounds::default()
    });
    for presented in &tokens {
        assert_eq!(reviewer.review(&token(presented)), admitted(AGENT));
        advance(&clock, Duration::from_secs(1));
    }
    assert_eq!(api.count(), 3);
    // The third entry pushed out the first, whose window ends soonest; the third is kept.
    assert_eq!(reviewer.review(&token(&tokens[2])), admitted(AGENT));
    assert_eq!(api.count(), 3);
    assert_eq!(reviewer.review(&token(&tokens[0])), admitted(AGENT));
    assert_eq!(api.count(), 4, "the cache kept more than its bound");
}

/// The pre-filter: a token whose unverified claims name another audience, no audience, or an
/// `exp` that has passed is refused without a review slot or a call to the API server.
#[test]
fn sa_the_pre_filter_refuses_on_the_tokens_own_claims_without_a_review() {
    let api = FakeApi::start();
    let (reviewer, _clock) = reviewer("sa-pre-filter", &api, &[AGENT]);
    let exp = later();
    let past = now_seconds() - 60;
    for (case, presented) in [
        (
            "another audience",
            jwt_claiming(&format!(
                r#"{{"aud":["kubernetes"],"exp":{exp},"sub":"{AGENT}"}}"#
            )),
        ),
        (
            "an audience as a prefix",
            jwt_claiming(&format!(
                r#"{{"aud":"{AUDIENCE}-x","exp":{exp},"sub":"{AGENT}"}}"#
            )),
        ),
        (
            "no audience",
            jwt_claiming(&format!(r#"{{"exp":{exp},"sub":"{AGENT}"}}"#)),
        ),
        (
            "an audience that is not text",
            jwt_claiming(&format!(r#"{{"aud":7,"exp":{exp},"sub":"{AGENT}"}}"#)),
        ),
        (
            "expired",
            jwt_claiming(&format!(
                r#"{{"aud":["{AUDIENCE}"],"exp":{past},"sub":"{AGENT}"}}"#
            )),
        ),
        ("not a JWT", "opaque-bearer-0123456789".to_owned()),
    ] {
        api.answer(&presented, admits(AGENT));
        assert_eq!(
            reviewer.review(&token(&presented)),
            Review::Rejected,
            "{case}"
        );
    }
    assert_eq!(
        api.count(),
        0,
        "a pre-filtered token reached the API server"
    );
}

/// The pre-filter only refuses: a forged token whose unverified claims name a listed account and
/// the audience is still reviewed, and the review's rejection stands.
#[test]
fn sa_a_forged_token_naming_a_listed_account_and_the_audience_is_still_reviewed_and_refused() {
    let api = FakeApi::start();
    let (reviewer, _clock) = reviewer("sa-forged", &api, &[AGENT]);
    let forged = jwt_claiming(&format!(
        r#"{{"aud":["{AUDIENCE}"],"exp":{},"sub":"{AGENT}"}}"#,
        later()
    ));
    assert_eq!(reviewer.review(&token(&forged)), Review::Rejected);
    assert_eq!(
        api.count(),
        1,
        "a token naming the audience was not reviewed"
    );
}

/// An answer that sets `status.error` is a rejection, kept as one, even with `authenticated`.
#[test]
fn sa_an_answer_with_status_error_set_is_a_rejection() {
    let api = FakeApi::start();
    let presented = jwt("erred", Some(later()));
    api.answer(&presented, Answer::Erred(AGENT));
    let (reviewer, _clock) = reviewer("sa-status-error", &api, &[AGENT]);
    assert_eq!(reviewer.review(&token(&presented)), Review::Rejected);
    assert_eq!(reviewer.review(&token(&presented)), Review::Rejected);
    assert_eq!(api.count(), 1, "the rejection was not kept");
}

/// A rejection never evicts an admission: with the cache full of admissions a new rejection is
/// not kept, and every admission stays.
#[test]
fn sa_a_rejection_never_evicts_an_admission() {
    let api = FakeApi::start();
    let admitted_tokens: Vec<String> = (0..2)
        .map(|n| jwt(&format!("kept-{n}"), Some(later())))
        .collect();
    for presented in &admitted_tokens {
        api.answer(presented, admits(AGENT));
    }
    let (reviewer, _clock) = reviewer("sa-eviction-kind", &api, &[AGENT]);
    let reviewer = reviewer.with_bounds(ReviewBounds {
        entries: 2,
        ..ReviewBounds::default()
    });
    for presented in &admitted_tokens {
        assert_eq!(reviewer.review(&token(presented)), admitted(AGENT));
    }
    for n in 0..4 {
        let refused = jwt(&format!("refused-{n}"), Some(later()));
        assert_eq!(reviewer.review(&token(&refused)), Review::Rejected);
    }
    assert_eq!(api.count(), 6);
    for presented in &admitted_tokens {
        assert_eq!(reviewer.review(&token(presented)), admitted(AGENT));
    }
    assert_eq!(api.count(), 6, "a rejection evicted an admission");
}

#[test]
fn sa_reviews_past_the_concurrency_bound_are_unavailable_at_once() {
    let api = FakeApi::start();
    let slow = jwt("slow", Some(later()));
    let quick = jwt("quick", Some(later()));
    api.answer(&slow, Answer::Hang(Duration::from_secs(3)));
    api.answer(&quick, admits(AGENT));
    let (reviewer, _) = reviewer("sa-concurrency", &api, &[AGENT]);
    let reviewer = Arc::new(reviewer.with_bounds(ReviewBounds {
        concurrent: 1,
        timeout: Duration::from_secs(2),
        ..ReviewBounds::default()
    }));
    let holding = Arc::clone(&reviewer);
    let slow_token = slow.clone();
    let held = thread::spawn(move || holding.review(&token(&slow_token)));
    let deadline = Instant::now() + WAIT;
    while api.count() == 0 {
        assert!(Instant::now() < deadline, "the first review never arrived");
        thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    assert_eq!(reviewer.review(&token(&quick)), Review::Unavailable);
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the refusal waited"
    );
    assert_eq!(api.count(), 1, "a review past the bound was sent");
    assert_eq!(held.join().unwrap(), Review::Unavailable);
    assert_eq!(reviewer.review(&token(&quick)), admitted(AGENT));
}

/// The bounds `docs/gateway.md` publishes for the review are the ones the binary serves with.
#[test]
fn sa_the_published_review_bounds_are_the_defaults() {
    let contract = include_str!("../../../docs/gateway.md");
    let section = contract
        .split_once("\n## Bounds of the service-account review\n")
        .expect("docs/gateway.md has no section for the review's bounds")
        .1
        .split("\n## ")
        .next()
        .unwrap();
    let rows: BTreeMap<String, u64> = section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            (
                cells[0].trim_matches('`').to_owned(),
                cells[2].parse().unwrap(),
            )
        })
        .collect();
    let bounds = ReviewBounds::default();
    let expected: BTreeMap<String, u64> = [
        (
            "review-timeout-ms",
            u64::try_from(bounds.timeout.as_millis()).unwrap(),
        ),
        (
            "concurrent-reviews",
            u64::try_from(bounds.concurrent).unwrap(),
        ),
        (
            "review-cache-entries",
            u64::try_from(bounds.entries).unwrap(),
        ),
        ("admission-cache-seconds", bounds.admission.as_secs()),
        ("rejection-cache-seconds", bounds.rejection.as_secs()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect();
    assert_eq!(rows, expected);
    assert_eq!(
        (
            bounds.timeout,
            bounds.concurrent,
            bounds.entries,
            bounds.admission,
            bounds.rejection
        ),
        (
            Duration::from_secs(5),
            8,
            1024,
            Duration::from_secs(60),
            Duration::from_secs(3)
        )
    );
}

// --- The document --------------------------------------------------------------------------------

fn refusal(text: &str) -> (String, String) {
    let refusal = llm_gateway_cli::parse(text).unwrap_err();
    (refusal.code().to_owned(), refusal.message().to_owned())
}

#[test]
fn sa_the_document_takes_the_keys_with_the_mounted_defaults_and_refuses_the_rest() {
    let model = "[hosted.open]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:1\"\nauth_kind = \"anonymous\"\n\n";
    let minimal = format!(
        "listen = \"127.0.0.1:0\"\ningress_auth = \"service-account-token\"\n\n{model}[token_review]\naudience = \"{AUDIENCE}\"\nprincipals = [\"{AGENT}\"]\n"
    );
    let deployment = llm_gateway_cli::parse(&minimal).unwrap();
    let review = deployment.token_review.as_ref().unwrap();
    assert_eq!(review.audience, AUDIENCE);
    assert_eq!(review.principals, vec![Principal::new(AGENT).unwrap()]);
    assert_eq!(review.api, "https://kubernetes.default.svc");
    assert_eq!(
        review.ca_file,
        Path::new("/var/run/secrets/kubernetes.io/serviceaccount/ca.crt")
    );
    assert_eq!(
        review.credential_file,
        Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token")
    );
    assert_eq!(
        deployment.gateway.ingress_auth,
        llm_gateway::IngressAuth::ServiceAccountToken
    );
    assert!(
        deployment.owner_secret_file.is_none(),
        "the owner secret is optional"
    );

    let replace = |find: &str, with: &str| minimal.replace(find, with);
    for (case, text, key) in [
        (
            "an empty allow-list",
            replace(&format!("[\"{AGENT}\"]"), "[]"),
            "token_review.principals",
        ),
        (
            "an account twice",
            replace(
                &format!("[\"{AGENT}\"]"),
                &format!("[\"{AGENT}\", \"{AGENT}\"]"),
            ),
            "token_review.principals",
        ),
        (
            "not a service account",
            replace(&format!("[\"{AGENT}\"]"), "[\"system:node:worker\"]"),
            "token_review.principals",
        ),
        (
            "an empty audience",
            replace(&format!("\"{AUDIENCE}\""), "\"\""),
            "token_review.audience",
        ),
        (
            "an audience with a space",
            replace(&format!("\"{AUDIENCE}\""), "\"llm gateway\""),
            "token_review.audience",
        ),
        (
            "a plain-text API",
            format!("{minimal}api = \"http://kubernetes.default.svc\"\n"),
            "token_review.api",
        ),
        (
            "an API with a path",
            format!("{minimal}api = \"https://kubernetes.default.svc/api\"\n"),
            "token_review.api",
        ),
        (
            "a relative CA file",
            format!("{minimal}ca_file = \"ca.crt\"\n"),
            "token_review.ca_file",
        ),
        (
            "a relative credential file",
            format!("{minimal}credential_file = \"token\"\n"),
            "token_review.credential_file",
        ),
        (
            "the mode without its table",
            minimal.split("[token_review]").next().unwrap().to_owned(),
            "token_review",
        ),
        (
            "the table without its mode",
            replace(
                "ingress_auth = \"service-account-token\"\n",
                "owner_secret_file = \"/run/owner\"\n",
            ),
            "token_review",
        ),
        (
            "the table under none",
            replace("\"service-account-token\"", "\"none\""),
            "token_review",
        ),
    ] {
        let (code, message) = refusal(&text);
        assert_eq!(code, "config:value", "{case}: {message}");
        assert!(message.starts_with(key), "{case}: {message}");
    }
    let (code, _) = refusal(&format!("{minimal}principal = \"{AGENT}\"\n"));
    assert_eq!(code, "config:schema", "the table is closed");
}

#[test]
fn sa_a_bearer_document_is_read_exactly_as_before() {
    let fixture = Fixture::new("sa-bearer-unchanged");
    let owner = fixture.owner_secret();
    let text = format!(
        "listen = \"127.0.0.1:0\"\nowner_secret_file = \"{}\"\n\n[hosted.open]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:1\"\nauth_kind = \"anonymous\"\n",
        owner.display()
    );
    let deployment = llm_gateway_cli::parse(&text).unwrap();
    assert_eq!(
        deployment.gateway.ingress_auth,
        llm_gateway::IngressAuth::Bearer
    );
    assert!(deployment.token_review.is_none());
    assert!(token_reviewer(&deployment).unwrap().is_none());
    // The owner secret is still required under bearer, with the same message.
    let (code, message) = refusal(&text.replace(
        &format!("owner_secret_file = \"{}\"\n", owner.display()),
        "",
    ));
    assert_eq!(code, "config:value");
    assert_eq!(
        message,
        "owner_secret_file is required unless ingress_auth is none"
    );
}

#[test]
fn sa_the_mounted_files_are_read_through_the_trusted_reader_at_start() {
    let api = FakeApi::start();
    let fixture = Fixture::new("sa-start-files");
    let (ca, own) = mounted(&fixture, &api);
    let compose = |ca: &Path, own: &Path| {
        let text = document(
            "127.0.0.1:0",
            None,
            "http://127.0.0.1:1",
            &token_review_table(&api.api(), ca, own, &[AGENT]),
        );
        token_reviewer(&llm_gateway_cli::parse(&text).unwrap()).map(|_| ())
    };
    assert!(compose(&ca, &own).is_ok());
    let missing = fixture.path("missing");
    for (case, ca, own, code, key) in [
        (
            "no CA",
            missing.clone(),
            own.clone(),
            "token-review:unreadable",
            "token_review.ca_file",
        ),
        (
            "no credential",
            ca.clone(),
            missing.clone(),
            "token-review:unreadable",
            "token_review.credential_file",
        ),
        (
            "a CA that holds no certificate",
            fixture.write("not-a-ca", b"not a certificate\n", 0o644),
            own.clone(),
            "token-review:ca",
            "token_review.ca_file",
        ),
        (
            "a writable credential",
            ca.clone(),
            fixture.write("writable", OWN_TOKEN.as_bytes(), 0o666),
            "token-review:unsafe-mode",
            "token_review.credential_file",
        ),
        (
            "an empty credential",
            ca.clone(),
            fixture.write("empty", b"\n", 0o600),
            "token-review:not-a-token",
            "token_review.credential_file",
        ),
    ] {
        let refusal = compose(&ca, &own).unwrap_err();
        assert_eq!(refusal.code(), code, "{case}: {refusal}");
        assert!(refusal.message().starts_with(key), "{case}: {refusal}");
        assert!(
            !refusal.message().contains(fixture.dir.to_str().unwrap()),
            "{case}: the refusal names a path"
        );
    }
}

// --- The shipped binary --------------------------------------------------------------------------

/// A loopback `http` endpoint answering every request with one chat completion and keeping the
/// head of each.
fn endpoint() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let heads: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&heads);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut connection) = connection else {
                return;
            };
            connection.set_read_timeout(Some(WAIT)).unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0_u8; 4096];
            let end = loop {
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end;
                }
                match connection.read(&mut chunk) {
                    Ok(0) | Err(_) => break 0,
                    Ok(read) => raw.extend_from_slice(&chunk[..read]),
                }
            };
            let head = String::from_utf8_lossy(&raw[..end]).into_owned();
            let length: usize = head
                .to_ascii_lowercase()
                .split("\r\n")
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse().ok())
                .unwrap_or(0);
            while raw.len() < end + 4 + length {
                match connection.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => raw.extend_from_slice(&chunk[..read]),
                }
            }
            seen.lock().unwrap().push(head);
            let body = r#"{"id":"chat-1","object":"chat.completion","model":"open","choices":[]}"#;
            drop(connection.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            ));
        }
    });
    (format!("http://{address}"), heads)
}

fn chat(address: SocketAddr, bearer: &str) -> (u16, String) {
    let body = r#"{"model":"open","messages":[]}"#;
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nhost: gateway\r\nauthorization: Bearer {bearer}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut answer = Vec::new();
    drop(stream.read_to_end(&mut answer));
    let text = String::from_utf8_lossy(&answer).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let code = text
        .split("\"code\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or("relayed")
        .to_owned();
    (status, code)
}

/// The shipped binary serving `config` with `RUST_LOG=debug`: the child, its standard error line
/// by line, the lines read until it listened, and the address it listens on.
fn serve(config: &std::path::Path) -> (Child, mpsc::Receiver<String>, Vec<String>, SocketAddr) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_b10x-llm-gateway"))
        .args(["--config", config.to_str().unwrap()])
        .env("RUST_LOG", "debug")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let mut written = Vec::new();
    let address: SocketAddr = loop {
        let line = lines.recv_timeout(WAIT).unwrap_or_else(|_| {
            drop(child.kill());
            panic!("the binary never listened: {written:?}")
        });
        written.push(line.clone());
        if let Some(address) = line.strip_prefix(LISTENING) {
            break address.parse().unwrap();
        }
    };
    (child, lines, written, address)
}

/// The shipped binary, under `service-account-token` with an owner secret: a reviewed token
/// and the owner's bearer each reach the model, an account off the list is refused, the usage
/// record names the admitted principal, and no line it writes holds a token, a part of one, its
/// own token or a review body.
#[test]
fn sa_the_binary_admits_a_reviewed_token_records_its_principal_and_logs_no_token() {
    let api = FakeApi::start();
    let fixture = Fixture::new("sa-binary");
    let (ca, own) = mounted(&fixture, &api);
    let owner = fixture.owner_secret();
    let (base_url, heads) = endpoint();
    let config = fixture.config(&document(
        "127.0.0.1:0",
        Some(&owner),
        &base_url,
        &token_review_table(&api.api(), &ca, &own, &[AGENT]),
    ));
    let agent = jwt("agent", Some(later()));
    let other = jwt("other", Some(later()));
    api.answer(&agent, admits(AGENT));
    api.answer(&other, admits(OTHER_AGENT));

    let (mut child, lines, mut written, address) = serve(&config);
    let results = [
        chat(address, &agent),
        chat(address, &agent),
        chat(address, OWNER_SECRET),
        chat(address, &other),
    ];
    drop(child.kill());
    drop(child.wait());
    while let Ok(line) = lines.recv_timeout(Duration::from_secs(2)) {
        written.push(line);
    }
    assert_eq!(
        results,
        [
            (200, "relayed".to_owned()),
            (200, "relayed".to_owned()),
            (200, "relayed".to_owned()),
            (401, "credential-rejected".to_owned()),
        ],
        "{written:#?}"
    );
    assert_eq!(
        api.count(),
        2,
        "the second call was reviewed again, or the owner's was"
    );
    let log = written.join("\n");
    let principal = format!("principal=\"{AGENT}\"");
    assert_eq!(
        written
            .iter()
            .filter(|line| line.contains("usage record") && line.contains(&principal))
            .count(),
        2,
        "{log}"
    );
    assert!(
        log.contains(&format!("principal=\"{OTHER_AGENT}\"")),
        "the refusal does not say whom it refused: {log}"
    );
    let mut withheld: Vec<String> = vec![OWN_TOKEN.to_owned(), OWNER_SECRET.to_owned()];
    for presented in [&agent, &other] {
        withheld.push(presented.clone());
        withheld.extend(presented.split('.').map(str::to_owned));
    }
    for secret in &withheld {
        assert!(
            !log.contains(secret.as_str()),
            "a token reached the log: {log}"
        );
    }
    assert!(
        !log.contains("TokenReview"),
        "a review body reached the log: {log}"
    );
    // No token reached the model.
    for head in heads.lock().unwrap().iter() {
        for secret in &withheld {
            assert!(
                !head.contains(secret.as_str()),
                "a token reached the model: {head}"
            );
        }
    }
}
