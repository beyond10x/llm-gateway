//! The binary's caller review (story:gateway-sa-token-ingress): a Kubernetes service-account
//! token checked with the cluster's TokenReview API, for a document with
//! `ingress_auth = "service-account-token"` (`spec/domains/deployment.yaml`,
//! `TokenReviewDeclaration`).
//!
//! Each review is one `POST` of a `TokenReview` naming the token and the configured audience to
//! `/apis/authentication.k8s.io/v1/tokenreviews`, over the binary's TLS client (`rustls` on
//! `ring`) trusting exactly the certificates of `token_review.ca_file`, authorised with the
//! gateway's own token from `token_review.credential_file`, read at every review so a rotated
//! token is used from the next one on. A caller is admitted only when the answer says
//! `authenticated`, names the configured audience among its `audiences`, and names a user on
//! `token_review.principals`; every other answer is a rejection or, when no review could be had,
//! unavailable. Never an admission by default.
//!
//! No token, no part of one and no review body reaches a log line, an error or a label: the
//! events below name a reason and, once the review named one, the principal.

use crate::{
    config::Deployment,
    refusal::{Refusal, StartupRefusal},
    relaying::PodConnector,
    tls::ProxyConnector,
    trusted::{self, Wiped},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use llm_gateway::{CallerReview, CallerToken, Principal, Review};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The path of the TokenReview API, `authentication.k8s.io/v1`.
pub const TOKEN_REVIEW_PATH: &str = "/apis/authentication.k8s.io/v1/tokenreviews";

/// The largest answer read from the TokenReview API; a longer one is malformed.
const MAX_ANSWER_BYTES: usize = 64 * 1024;

/// The bounds of the review. [`ReviewBounds::default`] is what the binary serves with and what
/// `docs/gateway.md` publishes ("Bounds of the service-account review"); a test may shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewBounds {
    /// One review, from opening the connection to the last byte of its answer.
    pub timeout: Duration,
    /// Reviews under way at once; one more is refused at once.
    pub concurrent: usize,
    /// Reviews kept, admissions and rejections together.
    pub entries: usize,
    /// The longest an admission is kept, never past the token's own expiry.
    pub admission: Duration,
    /// The longest a rejection is kept.
    pub rejection: Duration,
}

impl Default for ReviewBounds {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            concurrent: 8,
            entries: 1024,
            admission: Duration::from_secs(60),
            rejection: Duration::from_secs(3),
        }
    }
}

/// The clock the review reads, in milliseconds since the Unix epoch.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

fn wall_clock() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// One kept answer: until when, in milliseconds since the epoch, and what it was.
struct Kept {
    until_ms: u64,
    principal: Option<Principal>,
}

/// Why a review gave no answer, as the event names it.
#[derive(Debug, Clone, Copy)]
enum Failure {
    Credential,
    Unreachable,
    Timeout,
    Status,
    Malformed,
    Busy,
}

impl Failure {
    const fn label(self) -> &'static str {
        match self {
            Self::Credential => "own-token-unreadable",
            Self::Unreachable => "api-unreachable",
            Self::Timeout => "timeout",
            Self::Status => "non-success-status",
            Self::Malformed => "malformed-answer",
            Self::Busy => "too-many-reviews",
        }
    }
}

/// What the TokenReview API answered, read.
struct Answered {
    authenticated: bool,
    username: Option<String>,
    audiences: Vec<String>,
}

/// Reviews a caller's token with the cluster's TokenReview API.
pub struct TokenReviewer {
    authority: String,
    audience: String,
    principals: Vec<Principal>,
    credential_file: PathBuf,
    connector: Arc<dyn PodConnector>,
    bounds: ReviewBounds,
    clock: Clock,
    kept: Mutex<HashMap<[u8; 32], Kept>>,
    under_way: AtomicUsize,
}

impl fmt::Debug for TokenReviewer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenReviewer")
            .field("authority", &self.authority)
            .field("audience", &self.audience)
            .field("principals", &self.principals.len())
            .field("bounds", &self.bounds)
            .finish_non_exhaustive()
    }
}

impl TokenReviewer {
    /// The same reviewer under `bounds`.
    #[must_use]
    pub fn with_bounds(mut self, bounds: ReviewBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// The same reviewer reading `clock`, in milliseconds since the Unix epoch, for the cache's
    /// windows and a token's expiry. A test seam: the binary reads the wall clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// A kept answer for `key` at `now`; an expired one is dropped.
    fn kept(&self, key: &[u8; 32], now: u64) -> Option<Review> {
        let mut kept = self.kept.lock().ok()?;
        match kept.get(key) {
            Some(entry) if entry.until_ms > now => Some(
                entry
                    .principal
                    .clone()
                    .map_or(Review::Rejected, Review::Admitted),
            ),
            Some(_) => {
                kept.remove(key);
                None
            }
            None => None,
        }
    }

    /// Keeps `principal` (an admission, or a rejection with `None`) for `key` until `until_ms`,
    /// within the entry bound: expired entries go first, then the one whose window ends soonest.
    fn keep(&self, key: [u8; 32], until_ms: u64, principal: Option<Principal>, now: u64) {
        if self.bounds.entries == 0 || until_ms <= now {
            return;
        }
        let Ok(mut kept) = self.kept.lock() else {
            return;
        };
        if kept.len() >= self.bounds.entries && !kept.contains_key(&key) {
            kept.retain(|_, entry| entry.until_ms > now);
            while kept.len() >= self.bounds.entries {
                let Some(soonest) = kept
                    .iter()
                    .min_by_key(|(_, entry)| entry.until_ms)
                    .map(|(key, _)| *key)
                else {
                    break;
                };
                kept.remove(&soonest);
            }
        }
        kept.insert(
            key,
            Kept {
                until_ms,
                principal,
            },
        );
    }

    /// One review over the network, within the concurrency bound.
    fn ask(&self, token: &[u8]) -> Result<Answered, Failure> {
        if self.under_way.fetch_add(1, Ordering::SeqCst) >= self.bounds.concurrent {
            self.under_way.fetch_sub(1, Ordering::SeqCst);
            return Err(Failure::Busy);
        }
        let answered = self.exchange(token);
        self.under_way.fetch_sub(1, Ordering::SeqCst);
        answered
    }

    fn exchange(&self, token: &[u8]) -> Result<Answered, Failure> {
        let deadline = Instant::now() + self.bounds.timeout;
        let own = own_token(&self.credential_file).map_err(|refusal| {
            tracing::warn!(
                target: "ingress",
                refusal = refusal.code(),
                "service account review: the gateway's own token is unreadable"
            );
            Failure::Credential
        })?;
        let request = request(&self.authority, &self.audience, token, own.as_slice())
            .ok_or(Failure::Malformed)?;
        drop(own);
        let mut stream = self
            .connector
            .connect_probe(&self.authority, self.bounds.timeout)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    Failure::Timeout
                } else {
                    Failure::Unreachable
                }
            })?;
        if Instant::now() >= deadline {
            return Err(Failure::Timeout);
        }
        stream
            .write_all(request.as_slice())
            .map_err(|_| Failure::Unreachable)?;
        drop(request);
        let mut answer = Vec::new();
        let mut piece = [0_u8; 8 << 10];
        loop {
            if Instant::now() >= deadline {
                return Err(Failure::Timeout);
            }
            match stream.read(&mut piece) {
                Ok(0) => break,
                Ok(read) => {
                    answer.extend_from_slice(&piece[..read]);
                    if answer.len() > MAX_ANSWER_BYTES {
                        return Err(Failure::Malformed);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Err(Failure::Timeout);
                }
                // A server that closes without a TLS close_notify after a whole answer.
                Err(_) if !answer.is_empty() => break,
                Err(_) => return Err(Failure::Unreachable),
            }
        }
        let (status, body) = crate::vllm_stats::split_answer(&answer).ok_or(Failure::Malformed)?;
        if !matches!(status, 200 | 201) {
            return Err(Failure::Status);
        }
        answered(&body).ok_or(Failure::Malformed)
    }

    /// The decision on an answer, with the reason a refusal is logged with.
    fn decide(&self, answered: Answered) -> Result<Principal, (&'static str, Option<String>)> {
        if !answered.authenticated {
            return Err(("unauthenticated", None));
        }
        let username = answered.username;
        if !answered.audiences.iter().any(|aud| *aud == self.audience) {
            return Err(("audience-not-admitted", username));
        }
        let Some(principal) = username
            .as_deref()
            .and_then(|name| Principal::new(name).ok())
        else {
            return Err(("not-a-service-account", None));
        };
        if self.principals.contains(&principal) {
            Ok(principal)
        } else {
            Err(("principal-not-admitted", username))
        }
    }
}

impl CallerReview for TokenReviewer {
    fn review(&self, presented: &CallerToken) -> Review {
        let token = presented.expose();
        let now = (self.clock)();
        // A projected service-account token is a JWT; anything else is refused on its face. Its
        // `exp` is read unverified, only ever to refuse or to shorten a window, never to admit.
        let Some(expires_ms) = expiry_ms(token) else {
            refused("not-a-service-account-token", None);
            return Review::Rejected;
        };
        if expires_ms.is_some_and(|expires| expires <= now) {
            refused("expired", None);
            return Review::Rejected;
        }
        let key: [u8; 32] = Sha256::digest(token).into();
        if let Some(kept) = self.kept(&key, now) {
            return kept;
        }
        let answered = match self.ask(token) {
            Ok(answered) => answered,
            Err(failure) => {
                tracing::warn!(
                    target: "ingress",
                    reason = failure.label(),
                    "service account review unavailable"
                );
                return Review::Unavailable;
            }
        };
        let rejection_until = now.saturating_add(millis(self.bounds.rejection));
        match self.decide(answered) {
            Ok(principal) => {
                let until = now.saturating_add(millis(self.bounds.admission));
                let until = expires_ms.map_or(until, |expires| until.min(expires));
                tracing::debug!(
                    target: "ingress",
                    principal = principal.as_str(),
                    "service account admitted"
                );
                self.keep(key, until, Some(principal.clone()), now);
                Review::Admitted(principal)
            }
            Err((reason, principal)) => {
                refused(reason, principal.as_deref());
                self.keep(key, rejection_until, None, now);
                Review::Rejected
            }
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Logs a refusal with its reason, and the principal once a review named one. A principal names
/// an identity, never a credential; a username that is not a service account is not written.
fn refused(reason: &'static str, principal: Option<&str>) {
    let principal = principal.filter(|name| Principal::new(name).is_ok());
    tracing::warn!(
        target: "ingress",
        reason,
        principal,
        "service account token refused"
    );
}

/// `Some(exp)` in milliseconds for a token of three non-empty base64url segments whose middle
/// one is a JSON object (`Some(None)` when it has no numeric `exp`); `None` for anything else.
fn expiry_ms(token: &[u8]) -> Option<Option<u64>> {
    let mut segments = token.split(|byte| *byte == b'.');
    let (Some(header), Some(claims), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if header.is_empty() || signature.is_empty() {
        return None;
    }
    let decoded = Wiped::adopt(URL_SAFE_NO_PAD.decode(claims).ok()?);
    let claims: serde_json::Value = serde_json::from_slice(decoded.as_slice()).ok()?;
    let claims = claims.as_object()?;
    Some(
        claims
            .get("exp")
            .and_then(serde_json::Value::as_u64)
            .map(|exp| exp.saturating_mul(1000)),
    )
}

/// The request of one review: its head and its `TokenReview` body, in bytes that are overwritten
/// when dropped. `None` when the token is not text.
fn request(authority: &str, audience: &str, token: &[u8], own: &[u8]) -> Option<Wiped> {
    let token = std::str::from_utf8(token).ok()?;
    let body = serde_json::json!({
        "apiVersion": "authentication.k8s.io/v1",
        "kind": "TokenReview",
        "spec": {"token": token, "audiences": [audience]},
    });
    let body = Wiped::adopt(serde_json::to_vec(&body).ok()?);
    let head = format!(
        "POST {TOKEN_REVIEW_PATH} HTTP/1.1\r\nhost: {authority}\r\naccept: application/json\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\nauthorization: Bearer ",
        body.as_slice().len()
    );
    let mut request = Wiped::with_capacity(head.len() + own.len() + 4 + body.as_slice().len());
    request.extend(head.as_bytes());
    request.extend(own);
    request.extend(b"\r\n\r\n");
    request.extend(body.as_slice());
    Some(request)
}

/// The fields of a `TokenReview` answer the decision reads; `None` for anything that is not one.
fn answered(body: &[u8]) -> Option<Answered> {
    let review: serde_json::Value = serde_json::from_slice(body).ok()?;
    if review
        .get("kind")
        .is_some_and(|kind| kind.as_str() != Some("TokenReview"))
    {
        return None;
    }
    let status = review.get("status")?.as_object()?;
    let authenticated = status
        .get("authenticated")
        .map_or(Some(false), serde_json::Value::as_bool)?;
    let username = status
        .get("user")
        .and_then(|user| user.get("username"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    if authenticated && username.is_none() {
        return None;
    }
    let audiences = status
        .get("audiences")
        .and_then(serde_json::Value::as_array)
        .map(|audiences| {
            audiences
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(Answered {
        authenticated,
        username,
        audiences,
    })
}

/// The setting naming the gateway's own token, which its refusals name in place of the path.
const CREDENTIAL_FILE: &str = "token_review.credential_file";
/// The setting naming the CA bundle.
const CA_FILE: &str = "token_review.ca_file";

/// The gateway's own token from `path`, through the trusted-file reader: one printable ASCII
/// token of at most 16 KiB once trailing whitespace is trimmed.
fn own_token(path: &std::path::Path) -> Result<Wiped, Refusal> {
    let read = Wiped::adopt(
        trusted::read_named(path, CREDENTIAL_FILE, &trusted::TOKEN_REVIEW_CREDENTIAL)?.into_bytes(),
    );
    let material = read.as_slice().trim_ascii_end();
    if material.is_empty() || !material.iter().all(u8::is_ascii_graphic) {
        return Err(Refusal::new(
            StartupRefusal::TokenReviewNotAToken,
            format!("{CREDENTIAL_FILE}: the token must be one non-empty printable ASCII token"),
        ));
    }
    let mut token = Wiped::with_capacity(material.len());
    token.extend(material);
    Ok(token)
}

/// Every certificate of a PEM bundle, DER.
fn certificates(pem: &str) -> Vec<Vec<u8>> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut found = Vec::new();
    let mut rest = pem;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let Some(end) = after.find(END) else { break };
        let encoded: String = after[..end]
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(encoded) {
            found.push(der);
        }
        rest = &after[end + END.len()..];
    }
    found
}

/// The reviewer a document with `ingress_auth = "service-account-token"` serves with; `None` for
/// any other document. The CA bundle and the gateway's own token are read once here, so a file
/// that cannot be read refuses the start; the token is read again at every review.
///
/// # Errors
/// A `token-review:*` [`Refusal`], its message starting with the setting, never the path:
/// a trusted-file rule for either file, `token-review:not-a-token` for an own token that is not
/// one, `token-review:ca` for a CA file that holds no usable certificate.
pub fn token_reviewer(deployment: &Deployment) -> Result<Option<TokenReviewer>, Refusal> {
    let Some(config) = &deployment.token_review else {
        return Ok(None);
    };
    let pem = trusted::read_named(&config.ca_file, CA_FILE, &trusted::TOKEN_REVIEW_CA)?;
    let roots = certificates(&pem);
    let connector = ProxyConnector::trusting(&roots).map_err(|_| {
        Refusal::new(
            StartupRefusal::TokenReviewCa,
            format!("{CA_FILE}: holds no usable PEM certificate"),
        )
    })?;
    drop(own_token(&config.credential_file)?);
    let authority = config
        .api
        .strip_prefix("https://")
        .map(|rest| rest.strip_suffix('/').unwrap_or(rest))
        .unwrap_or_default()
        .to_owned();
    Ok(Some(TokenReviewer {
        authority,
        audience: config.audience.clone(),
        principals: config.principals.clone(),
        credential_file: config.credential_file.clone(),
        connector: Arc::new(connector),
        bounds: ReviewBounds::default(),
        clock: Arc::new(wall_clock),
        kept: Mutex::new(HashMap::new()),
        under_way: AtomicUsize::new(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::{answered, certificates, expiry_ms};

    #[test]
    fn a_token_is_read_for_its_expiry_only_when_it_is_shaped_as_a_jwt() {
        // {"exp":1800000000}
        assert_eq!(
            expiry_ms(b"aGVhZGVy.eyJleHAiOjE4MDAwMDAwMDB9.c2ln"),
            Some(Some(1_800_000_000_000))
        );
        // {"sub":"x"}
        assert_eq!(expiry_ms(b"aGVhZGVy.eyJzdWIiOiJ4In0.c2ln"), Some(None));
        for refused in [
            &b"opaque-token"[..],
            b"a.b",
            b"a.b.c.d",
            b".eyJzdWIiOiJ4In0.c2ln",
            b"aGVhZGVy.eyJzdWIiOiJ4In0.",
            b"aGVhZGVy.bm90LWpzb24.c2ln",
        ] {
            assert_eq!(
                expiry_ms(refused),
                None,
                "{}",
                String::from_utf8_lossy(refused)
            );
        }
    }

    #[test]
    fn an_answer_is_a_token_review_or_nothing() {
        let admitted = answered(
            br#"{"kind":"TokenReview","status":{"authenticated":true,"user":{"username":"u"},"audiences":["a"]}}"#,
        )
        .unwrap();
        assert!(admitted.authenticated);
        assert_eq!(admitted.username.as_deref(), Some("u"));
        assert_eq!(admitted.audiences, vec!["a".to_owned()]);
        let refused = answered(br#"{"status":{}}"#).unwrap();
        assert!(!refused.authenticated);
        for malformed in [
            &br#"{"kind":"Status","status":{"authenticated":true}}"#[..],
            br#"{"status":{"authenticated":true}}"#,
            br#"{"status":{"authenticated":"yes"}}"#,
            br#"{"kind":"TokenReview"}"#,
            b"<html>",
        ] {
            assert!(answered(malformed).is_none());
        }
    }

    #[test]
    fn a_pem_bundle_yields_each_certificate() {
        let pem = "junk\n-----BEGIN CERTIFICATE-----\nAAEC\nAw==\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nBAU=\n-----END CERTIFICATE-----\n";
        assert_eq!(certificates(pem), vec![vec![0, 1, 2, 3], vec![4, 5]]);
        assert!(certificates("no certificate").is_empty());
    }
}
