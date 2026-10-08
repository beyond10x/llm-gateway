//! Acceptance for `story:gateway-auth`, exercised over a real loopback socket.
//!
//! Unauthenticated requests are refused; an authenticated owner inspects routes without any
//! upstream credential appearing on the wire and without any secret source being consulted.

use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayConfig, GatewayHandle, Label, OwnerToken, RefusalCode,
    Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, RouteSummary,
    SharedSecretVerifier, ShutdownReport, TargetLimits, TargetProvenance, TargetRefusal,
    TargetSummary, Verdict, Wire,
};
use std::{
    collections::BTreeSet,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const OWNER_SECRET: &str = "owner-token-0123456789abcdef0123456789abcdef";
const CONTRACT: &str = include_str!("../../../docs/gateway.md");
/// Every source file of the crate. Checks that must be true of the crate read this, not a list.
const SOURCES: &[&str] = &[
    include_str!("../src/lib.rs"),
    include_str!("../src/auth.rs"),
    include_str!("../src/body.rs"),
    include_str!("../src/error.rs"),
    include_str!("../src/inventory.rs"),
    include_str!("../src/json.rs"),
    include_str!("../src/metrics.rs"),
    include_str!("../src/relay.rs"),
    include_str!("../src/server.rs"),
];
const OTHER_SECRET: &str = "other-token-0123456789abcdef0123456789abcdef";
/// The catalog's account names this reference; resolving it is never part of inspection.
const SECRET_REFERENCE: &str = "lab-llm-token";
const UPSTREAM_BASE_URL: &str = "https://models.example.invalid/v1";

/// Stands in for an injected credential source. The gateway never receives it: the embedding
/// resolves once at composition, so a served request cannot cause another read.
struct CountingSecretSource {
    reads: Arc<AtomicUsize>,
    material: Vec<u8>,
}

impl CountingSecretSource {
    fn new(material: &str) -> Self {
        Self {
            reads: Arc::new(AtomicUsize::new(0)),
            material: material.as_bytes().to_vec(),
        }
    }
    fn resolve(&self) -> Vec<u8> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.material.clone()
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

fn label(value: &str) -> Label {
    Label::new(value).unwrap()
}

fn one_route(index: usize) -> RouteSummary {
    RouteSummary::new(
        label(&format!("r{index}")),
        label(&format!("a{index}")),
        false,
        vec![target("t0", 0, TargetLimits::default())],
    )
    .unwrap()
}

fn target(id: &str, position: usize, limits: TargetLimits) -> TargetSummary {
    TargetSummary {
        target_id: label(id),
        position,
        provenance: TargetProvenance {
            protocol: label("chat-completions"),
            provider: label("my-lab"),
            account: label("remote"),
            endpoint: label("remote-models"),
            model: label("large"),
            binding_revision: label("rev-1"),
        },
        auth_kind: AuthKind::Bearer,
        billing_kind: BillingKind::Metered,
        limits,
    }
}

/// One route whose second target declares no limits at all: unknown must stay unknown.
fn inventory() -> RouteInventory {
    let route = RouteSummary::new(
        label("coding"),
        label("code"),
        false,
        vec![
            target(
                "coding-primary",
                0,
                TargetLimits {
                    context_window: Some(4096),
                    max_output_tokens: Some(1024),
                },
            ),
            target(
                "coding-secondary",
                1,
                TargetLimits {
                    context_window: None,
                    max_output_tokens: None,
                },
            ),
        ],
    )
    .unwrap();
    RouteInventory::new(vec![route]).unwrap()
}

struct Fixture {
    handle: Option<GatewayHandle>,
    addr: SocketAddr,
}

impl Fixture {
    fn start(ready: bool) -> Self {
        Self::start_with(ready, OWNER_SECRET, 8192)
    }

    fn start_with(ready: bool, secret: &str, max_head_bytes: usize) -> Self {
        Self::configured(ready, secret, |config| {
            config.max_head_bytes = max_head_bytes;
        })
    }

    fn configured(ready: bool, secret: &str, adjust: impl FnOnce(&mut GatewayConfig)) -> Self {
        Self::composed(ready, secret, None, adjust)
    }

    fn composed(
        ready: bool,
        secret: &str,
        relay: Option<Relay>,
        adjust: impl FnOnce(&mut GatewayConfig),
    ) -> Self {
        let source = CountingSecretSource::new(secret);
        let token = OwnerToken::new(source.resolve()).unwrap();
        let verifier = Arc::new(SharedSecretVerifier::new(token).unwrap());
        let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
        config.read_timeout = Duration::from_secs(5);
        adjust(&mut config);
        let handle = match relay {
            Some(relay) => Gateway::bind_with_relay(config, verifier, inventory(), relay),
            None => Gateway::bind(config, verifier, inventory()),
        }
        .unwrap();
        if ready {
            handle.mark_ready();
        }
        let addr = handle.local_addr();
        Self {
            handle: Some(handle),
            addr,
        }
    }

    fn handle(&self) -> &GatewayHandle {
        self.handle.as_ref().unwrap()
    }

    fn stop(mut self) -> ShutdownReport {
        self.handle.take().unwrap().shutdown()
    }

    fn send(&self, raw: &str) -> Response {
        exchange(self.addr, raw.as_bytes())
    }

    fn get(&self, path: &str, bearer: Option<&str>) -> Response {
        let auth = bearer.map_or(String::new(), |token| {
            format!("authorization: Bearer {token}\r\n")
        });
        self.send(&format!(
            "GET {path} HTTP/1.1\r\nhost: 127.0.0.1\r\n{auth}\r\n"
        ))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown();
        }
    }
}

#[derive(Debug)]
struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    raw: String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    fn code(&self) -> Option<String> {
        let marker = "\"code\":\"";
        let start = self.body.find(marker)? + marker.len();
        let rest = &self.body[start..];
        let end = rest.find('"')?;
        Some(rest[..end].to_string())
    }
}

fn exchange(addr: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((raw.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.to_string()))
        .collect();
    Response {
        status,
        headers,
        body: body.to_string(),
        raw: raw.clone(),
    }
}

#[test]
fn an_unauthenticated_inspection_is_refused_and_discloses_no_route() {
    let gateway = Fixture::start(true);
    let response = gateway.get("/v1/routes", None);
    assert_eq!(response.status, 401, "body was {}", response.body);
    assert_eq!(response.code().as_deref(), Some("credential-absent"));
    assert_eq!(response.header("www-authenticate"), Some("Bearer"));
    for disclosure in [
        "coding",
        "alias",
        "my-lab",
        "remote-models",
        "chat-completions",
        "4096",
    ] {
        assert!(
            !response.raw.contains(disclosure),
            "refusal disclosed {disclosure}: {}",
            response.raw
        );
    }
}

#[test]
fn a_rejected_or_malformed_credential_is_never_echoed_back() {
    let gateway = Fixture::start(true);
    for (raw, expected) in [
        (
            format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OTHER_SECRET}\r\n\r\n"
            ),
            "credential-rejected",
        ),
        (
            format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Basic {OTHER_SECRET}\r\n\r\n"
            ),
            "credential-malformed",
        ),
        (
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer \r\n\r\n".to_string(),
            "credential-malformed",
        ),
    ] {
        let response = gateway.send(&raw);
        assert_eq!(response.status, 401, "{}", response.raw);
        assert_eq!(response.code().as_deref(), Some(expected));
        assert!(!response.raw.contains(OTHER_SECRET), "{}", response.raw);
        assert!(!response.raw.contains(OWNER_SECRET), "{}", response.raw);
    }
}

#[test]
fn the_owner_inspects_routes_without_a_credential_or_a_secret_read() {
    let source = CountingSecretSource::new(OWNER_SECRET);
    let token = OwnerToken::new(source.resolve()).unwrap();
    assert_eq!(source.reads(), 1, "composition resolves exactly once");
    let verifier = Arc::new(SharedSecretVerifier::new(token).unwrap());
    let handle = Gateway::bind(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()),
        verifier,
        inventory(),
    )
    .unwrap();
    handle.mark_ready();
    let addr = handle.local_addr();

    for _ in 0..8 {
        let response = exchange(
            addr,
            format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
            )
            .as_bytes(),
        );
        assert_eq!(response.status, 200, "{}", response.raw);
        assert!(
            response.body.contains("\"alias\":\"code\""),
            "{}",
            response.body
        );
        assert!(response.body.contains("\"target_id\":\"coding-primary\""));
        for leak in [OWNER_SECRET, SECRET_REFERENCE, UPSTREAM_BASE_URL, "Bearer "] {
            assert!(!response.raw.contains(leak), "inspection leaked {leak}");
        }
    }
    assert_eq!(
        source.reads(),
        1,
        "serving inspections must not resolve a secret"
    );
    handle.shutdown();
}

#[test]
fn an_unreported_limit_stays_absent_and_never_becomes_zero() {
    let gateway = Fixture::start(true);
    let response = gateway.get("/v1/routes", Some(OWNER_SECRET));
    assert_eq!(response.status, 200, "{}", response.raw);
    assert!(response.body.contains("\"context_window\":4096"));
    assert!(
        !response.body.contains("\"context_window\":0"),
        "unknown became zero: {}",
        response.body
    );
    assert!(
        !response.body.contains("\"max_output_tokens\":0"),
        "unknown became zero: {}",
        response.body
    );
    // The second target declares nothing, so both keys are absent from its object.
    let second = response
        .body
        .split("\"target_id\":\"coding-secondary\"")
        .nth(1)
        .unwrap();
    assert!(!second.contains("context_window"), "{second}");
    assert!(!second.contains("max_output_tokens"), "{second}");
    // A digest nobody supplied is absent, not an empty string.
    assert!(
        !response.body.contains("config_digest"),
        "{}",
        response.body
    );
}

#[test]
fn a_single_route_is_inspectable_by_alias_and_an_unknown_alias_is_refused() {
    let gateway = Fixture::start(true);
    let found = gateway.get("/v1/routes/code", Some(OWNER_SECRET));
    assert_eq!(found.status, 200, "{}", found.raw);
    assert!(found.body.contains("\"route_id\":\"coding\""));
    let missing = gateway.get("/v1/routes/absent", Some(OWNER_SECRET));
    assert_eq!(missing.status, 404);
    assert_eq!(missing.code().as_deref(), Some("route-unknown"));
}

#[test]
fn health_and_readiness_are_distinct_and_need_no_credential() {
    let gateway = Fixture::start(false);
    let health = gateway.get("/health", None);
    assert_eq!(health.status, 200, "{}", health.raw);
    assert!(health.body.contains("\"status\":\"live\""));
    let unready = gateway.get("/ready", None);
    assert_eq!(unready.status, 503, "{}", unready.raw);
    assert!(unready.body.contains("\"status\":\"unready\""));
    gateway.handle().mark_ready();
    let ready = gateway.get("/ready", None);
    assert_eq!(ready.status, 200, "{}", ready.raw);
    assert!(ready.body.contains("\"status\":\"ready\""));
    // A probe path with a write method is not a probe; it is authenticated like anything else.
    let posted = gateway.send("POST /health HTTP/1.1\r\nhost: h\r\n\r\n");
    assert_eq!(posted.status, 401);
    assert_eq!(posted.code().as_deref(), Some("credential-absent"));
}

#[test]
fn a_head_request_returns_the_headers_without_a_body() {
    let gateway = Fixture::start(true);
    let response = gateway.send(&format!(
        "HEAD /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
    ));
    assert_eq!(response.status, 200, "{}", response.raw);
    assert!(response.body.is_empty(), "{}", response.body);
    let length: usize = response.header("content-length").unwrap().parse().unwrap();
    assert!(length > 0, "HEAD must still report the length");
}

#[test]
fn graceful_shutdown_drains_readiness_then_stops_accepting() {
    let gateway = Fixture::start(true);
    let addr = gateway.addr;
    assert_eq!(gateway.get("/v1/routes", Some(OWNER_SECRET)).status, 200);
    gateway.handle().begin_drain();
    // Draining removes the gateway from rotation but keeps serving what arrives.
    let ready = gateway.get("/ready", None);
    assert_eq!(ready.status, 503, "{}", ready.raw);
    assert_eq!(gateway.get("/health", None).status, 200);
    assert_eq!(gateway.get("/v1/routes", Some(OWNER_SECRET)).status, 200);

    let idle = Fixture::start(true).stop();
    assert_eq!(idle.accepted, 0, "{idle:?}");
    assert_eq!(idle.accepted, idle.completed, "{idle:?}");
    assert_eq!(idle.in_flight_at_signal, 0, "{idle:?}");

    let report = gateway.stop();
    assert!(report.accepted >= 4, "{report:?}");
    assert_eq!(report.accepted, report.completed, "{report:?}");
    assert!(
        TcpStream::connect(addr).is_err(),
        "the listener still accepts after shutdown"
    );
}

#[test]
fn the_owner_credential_is_redacted_in_every_debug_rendering() {
    let token = OwnerToken::new(OWNER_SECRET.as_bytes().to_vec()).unwrap();
    assert_eq!(format!("{token:?}"), "OwnerToken([REDACTED])");
    let verifier = SharedSecretVerifier::new(token).unwrap();
    assert!(!format!("{verifier:?}").contains(OWNER_SECRET));
    let config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    assert!(!format!("{config:?}").contains(OWNER_SECRET));
    let inventory = inventory();
    assert!(!format!("{inventory:?}").contains(OWNER_SECRET));
}

/// The provocation for one refusal code. The match below is exhaustive, so a new variant of
/// `RefusalCode` cannot be added without giving it a request that actually provokes it.
struct Provocation {
    ready: bool,
    max_head_bytes: usize,
    max_concurrent_requests: u64,
    raw: String,
    /// Whether the gateway is composed with [`provoking_relay`].
    relayed: bool,
}

/// A target nothing can connect to.
struct Unreachable;

impl RelayTarget for Unreachable {
    fn authority(&self) -> &'static str {
        "unreachable.invalid:8000"
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::ErrorKind::ConnectionRefused.into())
    }
}

/// Hands out an unreachable target for `down`, reports `cold` still starting, and has none for
/// anything else.
struct ProvokingTargets;

impl RelayTargets for ProvokingTargets {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        match alias {
            "down" => Ok(Box::new(Unreachable)),
            "cold" => Err(TargetRefusal::ColdStart),
            _ => Err(TargetRefusal::Unavailable),
        }
    }

    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// Three chat-only models: `down`, whose target cannot be reached, `cold`, whose target is
/// still starting, and `none`, which has none.
fn provoking_relay() -> Relay {
    let model = |alias: &str| RelayModel::new(label(alias), label("served"), vec![Wire::Chat]);
    Relay::new(
        vec![
            model("down").unwrap(),
            model("cold").unwrap(),
            model("none").unwrap(),
        ],
        Arc::new(ProvokingTargets),
    )
    .unwrap()
}

/// An authenticated relay of `body` on `path`.
fn relay_request(path: &str, body: &str) -> String {
    format!(
        "POST {path} HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn provoke(code: RefusalCode) -> Provocation {
    let plain = |raw: &str| Provocation {
        ready: true,
        max_head_bytes: 8192,
        max_concurrent_requests: 64,
        raw: raw.to_string(),
        relayed: false,
    };
    let owned = |raw: String| Provocation {
        ready: true,
        max_head_bytes: 8192,
        max_concurrent_requests: 64,
        raw,
        relayed: false,
    };
    let relayed = |path: &str, body: &str| Provocation {
        ready: true,
        max_head_bytes: 8192,
        max_concurrent_requests: 64,
        raw: relay_request(path, body),
        relayed: true,
    };
    let chat = "/v1/chat/completions";
    match code {
        RefusalCode::BodyTooLarge => Provocation {
            raw: format!(
                "POST {chat} HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: 33554433\r\n\r\n"
            ),
            ..relayed(chat, "")
        },
        // Two of the 64 declared bytes, then nothing: the body stalls past the read timeout.
        RefusalCode::BodyIncomplete => Provocation {
            raw: format!(
                "POST {chat} HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\ncontent-length: 64\r\n\r\n{{}}"
            ),
            ..relayed(chat, "")
        },
        RefusalCode::BodyNotJson => relayed(chat, "nope"),
        RefusalCode::ModelAbsent => relayed(chat, "{}"),
        RefusalCode::ModelUnknown => relayed(chat, "{\"model\":\"other\"}"),
        RefusalCode::WireNotServed => relayed("/v1/messages", "{\"model\":\"none\"}"),
        RefusalCode::TargetUnavailable => relayed(chat, "{\"model\":\"none\"}"),
        RefusalCode::ModelColdStart => relayed(chat, "{\"model\":\"cold\"}"),
        RefusalCode::UpstreamFailed => relayed(chat, "{\"model\":\"down\"}"),
        RefusalCode::CredentialAbsent => plain("GET /v1/routes HTTP/1.1\r\nhost: h\r\n\r\n"),
        RefusalCode::CredentialRejected => owned(format!(
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OTHER_SECRET}\r\n\r\n"
        )),
        RefusalCode::CredentialMalformed => {
            plain("GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Token abc\r\n\r\n")
        }
        RefusalCode::MethodNotAllowed => owned(format!(
            "DELETE /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        )),
        RefusalCode::BodyNotAllowed => owned(format!(
            "GET /v1/routes HTTP/1.1\r\nhost: h\r\ncontent-length: 3\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\nabc"
        )),
        RefusalCode::PathUnknown => owned(format!(
            "GET /v1/nothing HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        )),
        RefusalCode::RouteUnknown => owned(format!(
            "GET /v1/routes/absent HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
        )),
        RefusalCode::RequestMalformed => plain("not-a-request-line\r\n\r\n"),
        RefusalCode::RequestTooLarge => Provocation {
            ready: true,
            max_head_bytes: 512,
            max_concurrent_requests: 64,
            raw: format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nx-pad: {}\r\n\r\n",
                "p".repeat(2048)
            ),
            relayed: false,
        },
        // A bound of zero makes every connection one too many: the overload path, without a race.
        RefusalCode::Overloaded => Provocation {
            ready: true,
            max_head_bytes: 8192,
            max_concurrent_requests: 0,
            raw: format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
            ),
            relayed: false,
        },
        RefusalCode::Unavailable => Provocation {
            ready: false,
            max_head_bytes: 8192,
            max_concurrent_requests: 64,
            raw: format!(
                "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n"
            ),
            relayed: false,
        },
    }
}

#[test]
fn every_refusal_code_has_a_request_that_provokes_it() {
    for &code in RefusalCode::ALL {
        let provocation = provoke(code);
        let relay = provocation.relayed.then(provoking_relay);
        let gateway = Fixture::composed(provocation.ready, OWNER_SECRET, relay, |config| {
            config.max_head_bytes = provocation.max_head_bytes;
            config.max_concurrent_requests = provocation.max_concurrent_requests;
        });
        let response = gateway.send(&provocation.raw);
        assert_eq!(
            response.code().as_deref(),
            Some(code.wire()),
            "{:?} was not provoked: {}",
            code,
            response.raw
        );
        assert_eq!(response.status, code.status(), "{}", response.raw);
        assert_eq!(response.header("cache-control"), Some("no-store"));
        // Only a cold start tells the client when to come back (row W6).
        let retry_after = (code == RefusalCode::ModelColdStart).then_some("30");
        assert_eq!(
            response.header("retry-after"),
            retry_after,
            "{code:?}: {}",
            response.raw
        );
        assert!(
            response.body.contains(code.reason()),
            "the fixed diagnostic is missing: {}",
            response.body
        );
    }
}

/// The refusal rows of `docs/gateway.md`, as `(code, status, message)`.
fn published_refusal_table() -> Vec<(String, u16, String)> {
    let section = CONTRACT
        .split_once("\n## Refusals\n")
        .expect("docs/gateway.md has no Refusals section")
        .1
        .split("\n## ")
        .next()
        .unwrap_or_default();
    section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 3, "unexpected refusal row: {line}");
            (
                cells[0].trim_matches('`').to_string(),
                cells[1]
                    .parse()
                    .unwrap_or_else(|_| panic!("unparsable status in: {line}")),
                cells[2].to_string(),
            )
        })
        .collect()
}

#[test]
fn the_published_refusal_table_matches_the_codes_the_crate_can_emit() {
    // `RefusalCode::ALL`, `wire`, `status` and `reason` are generated from one list by the
    // `refusal_codes!` macro in src/error.rs, so a variant missing from any of them is now a
    // compile error. That replaces the earlier completeness check, which parsed src/error.rs by
    // line and could be fooled by a brace inside a doc comment. What remains to check at
    // runtime is the link to the published document -- including its status column, which
    // nothing compared before, so six of ten statuses were asserted only against themselves.
    let published = published_refusal_table();
    let mut wires = BTreeSet::new();
    for &code in RefusalCode::ALL {
        assert!(
            wires.insert(code.wire()),
            "duplicate wire code {}",
            code.wire()
        );
        let row = published
            .iter()
            .find(|(wire, _, _)| wire == code.wire())
            .unwrap_or_else(|| {
                panic!(
                    "docs/gateway.md does not name the refusal code {}",
                    code.wire()
                )
            });
        assert_eq!(
            row.1,
            code.status(),
            "docs/gateway.md publishes status {} for {}, the crate answers {}",
            row.1,
            code.wire(),
            code.status()
        );
        assert_eq!(
            row.2,
            code.reason(),
            "docs/gateway.md publishes a different message for {}",
            code.wire()
        );
    }
    let stale: Vec<&String> = published
        .iter()
        .map(|(wire, _, _)| wire)
        .filter(|wire| !wires.contains(wire.as_str()))
        .collect();
    assert!(
        stale.is_empty(),
        "docs/gateway.md publishes refusal codes the crate cannot emit: {stale:?}"
    );
    assert_eq!(published.len(), RefusalCode::ALL.len());
}

#[test]
fn a_drain_cannot_be_undone_by_marking_ready_again() {
    let gateway = Fixture::start(true);
    assert_eq!(gateway.get("/ready", None).status, 200);
    gateway.handle().begin_drain();
    assert_eq!(gateway.get("/ready", None).status, 503);
    // An embedding that re-marks readiness on a timer must not return a draining gateway to
    // rotation.
    gateway.handle().mark_ready();
    let after = gateway.get("/ready", None);
    assert_eq!(after.status, 503, "{}", after.raw);
    assert!(
        after.body.contains("\"status\":\"unready\""),
        "{}",
        after.body
    );
    // It still serves what arrives, which is the point of a drain.
    assert_eq!(gateway.get("/v1/routes", Some(OWNER_SECRET)).status, 200);
}

#[test]
fn liveness_is_answered_before_load_is_shed() {
    let gateway = Fixture::configured(true, OWNER_SECRET, |config| {
        config.max_concurrent_requests = 0;
    });
    let health = gateway.get("/health", None);
    assert_eq!(health.status, 200, "{}", health.raw);
    assert!(health.body.contains("\"status\":\"live\""));
    assert_eq!(gateway.get("/ready", None).status, 200);
    // Everything that is not a probe is shed, and says so as overload rather than as unready.
    let shed = gateway.get("/v1/routes", Some(OWNER_SECRET));
    assert_eq!(shed.status, 503, "{}", shed.raw);
    assert_eq!(shed.code().as_deref(), Some("overloaded"));
}

/// How a published bound behaves at its own number.
enum Direction {
    /// The number is admitted, one more is refused.
    Maximum,
    /// The number is admitted, one less is refused.
    Minimum,
    /// Only the number itself matches.
    Exact,
}

struct Bound {
    name: &'static str,
    value: u64,
    direction: Direction,
    admits: Box<dyn Fn(u64) -> bool>,
}

/// The bounds rows of `docs/gateway.md`, as `(bound, constant, value)`.
fn published_bounds() -> Vec<(String, String, u64)> {
    let section = CONTRACT
        .split_once("\n## Bounds\n")
        .expect("docs/gateway.md has no Bounds section")
        .1
        .split("\n## ")
        .next()
        .unwrap_or_default();
    section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            assert_eq!(cells.len(), 3, "unexpected bound row: {line}");
            (
                cells[0].trim_matches('`').to_string(),
                cells[1].trim_matches('`').to_string(),
                cells[2]
                    .parse()
                    .unwrap_or_else(|_| panic!("unparsable bound in: {line}")),
            )
        })
        .collect()
}

/// Every numeric constant the crate's own source declares, as `(name, value)`.
fn numeric_constants_in_the_source() -> Vec<(String, u64)> {
    let mut found = Vec::new();
    for source in SOURCES {
        for line in source.lines() {
            let line = line.trim();
            // Every visibility a constant can be declared with: a `pub(crate)` bound is as real
            // as a private one, and a scan that skipped it would never ask for its row.
            let Some(rest) = line
                .strip_prefix("pub const ")
                .or_else(|| line.strip_prefix("pub(crate) const "))
                .or_else(|| line.strip_prefix("const "))
            else {
                continue;
            };
            let Some((name, tail)) = rest.split_once(": ") else {
                continue;
            };
            let Some((kind, value)) = tail.split_once(" = ") else {
                continue;
            };
            if kind != "usize" && kind != "u64" {
                continue;
            }
            // Digit separators are part of the literal: `33_554_432` is a bound like `4096`.
            if let Ok(value) = value.trim_end_matches(';').replace('_', "").parse() {
                found.push((name.to_string(), value));
            }
        }
    }
    found
}

/// Constants that are not bounds on anything a caller can observe. Naming one here is the only
/// way to keep it out of the published table, and it has to carry a reason.
const NOT_A_BOUND: &[(&str, &str)] = &[
    (
        "READ_CHUNK",
        "the size of one internal read, not a limit on any request",
    ),
    (
        "RELAY_CHUNK",
        "the size of one internal read while relaying, not a limit on any request or answer",
    ),
    (
        "DEFAULT_READ_TIMEOUT_SECONDS",
        "a timeout rather than a size bound; enforcement is measured by \
         a_stalled_head_is_refused_when_the_read_timeout_expires",
    ),
    (
        "COLD_START_RETRY_AFTER_SECONDS",
        "the delay a model-cold-start refusal asks the client to wait (row W6), a hint rather \
         than a limit; its header is measured by \
         w6_a_model_still_starting_past_its_hold_budget_is_model_cold_start_with_retry_after_30",
    ),
];

/// The table can only be complete if it is compared against the source. Comparing it against
/// another hand-written list in this file — which is what it did — cannot see a bound that is
/// absent from both, and that is how the 64-character digest bound stayed unpublished.
#[test]
fn every_numeric_bound_in_the_source_is_published() {
    let declared = numeric_constants_in_the_source();
    assert!(
        declared.len() >= 8,
        "the constant scan found almost nothing, so it is not reading the source: {declared:?}"
    );
    let published = published_bounds();
    for (name, value) in &declared {
        if let Some((_, reason)) = NOT_A_BOUND.iter().find(|(exempt, _)| exempt == name) {
            assert_ne!(*reason, "");
            continue;
        }
        let row = published
            .iter()
            .find(|(_, constant, _)| constant == name)
            .unwrap_or_else(|| {
                panic!(
                    "src declares the bound {name} = {value} and docs/gateway.md does not \
                     publish it; if it is not a bound, name it in NOT_A_BOUND with a reason"
                )
            });
        assert_eq!(
            row.2, *value,
            "docs/gateway.md publishes {} for {name}, the source says {value}",
            row.2
        );
    }
    let names: BTreeSet<&str> = declared.iter().map(|(name, _)| name.as_str()).collect();
    for (_, constant, _) in &published {
        assert!(
            names.contains(constant.as_str()),
            "docs/gateway.md publishes the constant {constant}, which the source does not declare"
        );
    }
}

fn enforced_bounds() -> Vec<Bound> {
    let bytes = |count: u64| vec![b'a'; usize::try_from(count).unwrap()];
    let targets = |count: u64| {
        (0..usize::try_from(count).unwrap())
            .map(|position| {
                let mut summary = target(
                    &format!("t{position}"),
                    position,
                    TargetLimits {
                        context_window: None,
                        max_output_tokens: None,
                    },
                );
                summary.position = position;
                summary
            })
            .collect::<Vec<_>>()
    };
    vec![
        Bound {
            name: "owner-credential-bytes",
            value: 4096,
            direction: Direction::Maximum,
            admits: Box::new(move |n| OwnerToken::new(bytes(n)).is_ok()),
        },
        Bound {
            name: "shared-secret-bytes",
            value: 32,
            direction: Direction::Minimum,
            admits: Box::new(move |n| {
                SharedSecretVerifier::new(OwnerToken::new(bytes(n)).unwrap()).is_ok()
            }),
        },
        Bound {
            name: "label-bytes",
            value: 256,
            direction: Direction::Maximum,
            admits: Box::new(|n| Label::new("x".repeat(usize::try_from(n).unwrap())).is_ok()),
        },
        Bound {
            name: "targets-per-route",
            value: 64,
            direction: Direction::Maximum,
            admits: Box::new(move |n| {
                RouteSummary::new(label("r"), label("a"), false, targets(n)).is_ok()
            }),
        },
        Bound {
            name: "routes",
            value: 4096,
            direction: Direction::Maximum,
            admits: Box::new(move |n| {
                RouteInventory::new(
                    (0..usize::try_from(n).unwrap())
                        .map(one_route)
                        .collect::<Vec<_>>(),
                )
                .is_ok()
            }),
        },
        Bound {
            name: "digest-characters",
            value: 64,
            direction: Direction::Exact,
            admits: Box::new(move |n| {
                RouteInventory::new(vec![one_route(0)])
                    .unwrap()
                    .with_config_digest(&"0".repeat(usize::try_from(n).unwrap()))
                    .is_ok()
            }),
        },
        // Measured by enforcement over a real socket, on the default configuration. Reading
        // `GatewayConfig::new(..).max_head_bytes` here — which is what this did — leaves the
        // head arithmetic free to change with the suite green.
        Bound {
            name: "request-head-bytes",
            value: 8192,
            direction: Direction::Maximum,
            admits: Box::new(|n| a_head_of_exactly(usize::try_from(n).unwrap()) == 200),
        },
        // Likewise: the comparison in `decide` is what enforces this, not the default field.
        Bound {
            name: "concurrent-requests",
            value: 64,
            direction: Direction::Maximum,
            admits: Box::new(|n| the_nth_concurrent_connection(usize::try_from(n).unwrap()) == 200),
        },
        // Over a real socket, with the whole body sent: what is measured is the relay reading
        // it, not the declared length alone.
        Bound {
            name: "request-body-bytes",
            value: 33_554_432,
            direction: Direction::Maximum,
            admits: Box::new(|n| a_relayed_body_of_exactly(usize::try_from(n).unwrap()).is_none()),
        },
    ]
}

/// Sends an authenticated relay whose body is exactly `total` bytes and returns the refusal it
/// was answered with if that refusal is `body-too-large`. A body within the bound is read and
/// then refused as not JSON, which is the other answer.
fn a_relayed_body_of_exactly(total: usize) -> Option<String> {
    let gateway = Fixture::composed(true, OWNER_SECRET, Some(provoking_relay()), |_| {});
    let raw = relay_request("/v1/chat/completions", &"a".repeat(total));
    let code = gateway.send(&raw).code();
    assert!(
        matches!(code.as_deref(), Some("body-too-large" | "body-not-json")),
        "{code:?}"
    );
    code.filter(|code| code == "body-too-large")
}

/// Sends a request head of exactly `total` bytes to a default-configured gateway and returns the
/// status. The head is otherwise valid and authenticated, so only its length can refuse it.
fn a_head_of_exactly(total: usize) -> u16 {
    let gateway = Fixture::configured(true, OWNER_SECRET, |_| {});
    let prefix = format!(
        "GET /v1/routes HTTP/1.1\r\nhost: h\r\nauthorization: Bearer {OWNER_SECRET}\r\nx-pad: "
    );
    let suffix = "\r\n\r\n";
    let raw = format!(
        "{prefix}{}{suffix}",
        "p".repeat(total - prefix.len() - suffix.len())
    );
    assert_eq!(raw.len(), total);
    gateway.send(&raw).status
}

/// Occupies `n - 1` slots with connections whose head is not yet terminated, then returns the
/// status the `n`-th concurrent connection is answered with.
fn the_nth_concurrent_connection(n: usize) -> u16 {
    let gateway = Fixture::configured(true, OWNER_SECRET, |_| {});
    let mut held = Vec::new();
    for _ in 0..n - 1 {
        let mut stream = TcpStream::connect(gateway.addr).unwrap();
        stream
            .write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
            .unwrap();
        stream.flush().unwrap();
        held.push(stream);
    }
    std::thread::sleep(Duration::from_millis(400));
    let status = gateway.get("/v1/routes", Some(OWNER_SECRET)).status;
    drop(held);
    status
}

/// A gateway whose read timeout is short answers a head that never arrives, rather than holding
/// the connection open. This measures the timeout's enforcement; the default value is asserted
/// alongside it and is deliberately not published as a bound.
#[test]
fn a_stalled_head_is_refused_when_the_read_timeout_expires() {
    assert_eq!(
        GatewayConfig::new("127.0.0.1:0".parse().unwrap()).read_timeout,
        Duration::from_secs(10),
        "the default this crate documents"
    );
    let gateway = Fixture::configured(true, OWNER_SECRET, |config| {
        config.read_timeout = Duration::from_millis(150);
    });
    let mut stream = TcpStream::connect(gateway.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(b"GET /v1/routes HTTP/1.1\r\nhost: h\r\n")
        .unwrap();
    stream.flush().unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    assert!(
        raw.contains("request-malformed"),
        "a head that never arrives must be refused, not held: {raw}"
    );
}

/// The read timeout is one deadline for the whole head. A peer that sends a byte every 50 ms
/// never lets a single read time out, so a timeout applied per read would hold the connection,
/// and a graceful stop with it, for as long as the peer keeps trickling.
#[test]
fn a_trickled_head_is_refused_when_the_read_timeout_expires() {
    let gateway = Fixture::configured(true, OWNER_SECRET, |config| {
        config.read_timeout = Duration::from_millis(300);
    });
    let mut stream = TcpStream::connect(gateway.addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(b"GET /v1/routes HTTP/1.1\r\n").unwrap();
    let mut trickler = stream.try_clone().unwrap();
    let trickle = std::thread::spawn(move || {
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(50));
            if trickler.write_all(b"x").is_err() {
                return;
            }
        }
    });
    let started = Instant::now();
    let mut raw = Vec::new();
    drop(stream.read_to_end(&mut raw));
    let elapsed = started.elapsed();
    drop(trickle.join());
    let raw = String::from_utf8_lossy(&raw).into_owned();
    assert!(
        raw.contains("request-malformed"),
        "a head still trickling at the deadline must be refused: {raw}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "the head was held for {elapsed:?}, past its 300 ms deadline"
    );
}

/// The response is written under its own deadline. A peer that asks for a large inventory and
/// never reads it fills both socket buffers; without the deadline the connection, and the
/// graceful stop that joins it, would wait for the peer forever.
#[test]
fn a_peer_that_never_reads_its_answer_cannot_hold_the_stop() {
    let long = |prefix: &str, index: usize| label(&format!("{prefix}{index}-{}", "x".repeat(240)));
    // 4096 routes of four targets with six 250-byte labels each: tens of megabytes, more than
    // the loopback socket buffers hold.
    let routes = (0..4096)
        .map(|index| {
            let targets = (0..4)
                .map(|position| TargetSummary {
                    target_id: long("t", position),
                    position,
                    provenance: TargetProvenance {
                        protocol: long("p", index),
                        provider: long("v", index),
                        account: long("c", index),
                        endpoint: long("e", index),
                        model: long("m", index),
                        binding_revision: long("b", index),
                    },
                    auth_kind: AuthKind::Bearer,
                    billing_kind: BillingKind::Metered,
                    limits: TargetLimits::default(),
                })
                .collect();
            RouteSummary::new(long("r", index), long("a", index), false, targets).unwrap()
        })
        .collect();
    let mut config = GatewayConfig::new("127.0.0.1:0".parse().unwrap());
    config.read_timeout = Duration::from_millis(300);
    let verifier =
        Arc::new(SharedSecretVerifier::new(OwnerToken::new(OWNER_SECRET.into()).unwrap()).unwrap());
    let handle = Gateway::bind(config, verifier, RouteInventory::new(routes).unwrap()).unwrap();
    handle.mark_ready();
    let mut stream = TcpStream::connect(handle.local_addr()).unwrap();
    stream
        .write_all(
            format!("GET /v1/routes HTTP/1.1\r\nauthorization: Bearer {OWNER_SECRET}\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    // Let the gateway start writing into buffers nobody drains.
    std::thread::sleep(Duration::from_millis(200));
    let (sender, receiver) = std::sync::mpsc::channel();
    // The send fails only once the receiver gave up, which the expect below reports.
    std::thread::spawn(move || sender.send(handle.shutdown()));
    let report = receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("a peer that never reads its answer held the graceful stop");
    assert_eq!(report.accepted, 1);
    assert_eq!(report.completed, 1);
    drop(stream);
}

/// A bound asserted against its own constant asserts nothing: lowering the constant moves the
/// assertion with it. Every bound is therefore measured at the literal number `docs/gateway.md`
/// publishes, and at one past it.
#[test]
fn every_published_bound_flips_at_exactly_the_published_number() {
    let published = published_bounds();
    let enforced = enforced_bounds();
    let published_names: BTreeSet<&str> =
        published.iter().map(|(name, _, _)| name.as_str()).collect();
    let enforced_names: BTreeSet<&str> = enforced.iter().map(|bound| bound.name).collect();
    assert_eq!(
        published_names, enforced_names,
        "docs/gateway.md and the measured bounds do not name the same set"
    );

    for bound in &enforced {
        let (_, _, value) = published
            .iter()
            .find(|(name, _, _)| name == bound.name)
            .unwrap();
        assert_eq!(
            *value, bound.value,
            "docs/gateway.md publishes {value} for {}",
            bound.name
        );
        assert!(
            (bound.admits)(bound.value),
            "{} refuses its own published value {}",
            bound.name,
            bound.value
        );
        match bound.direction {
            Direction::Maximum => assert!(
                !(bound.admits)(bound.value + 1),
                "{} admits {} , one past its published maximum",
                bound.name,
                bound.value + 1
            ),
            Direction::Minimum => assert!(
                !(bound.admits)(bound.value - 1),
                "{} admits {}, one below its published minimum",
                bound.name,
                bound.value - 1
            ),
            Direction::Exact => {
                assert!(!(bound.admits)(bound.value + 1));
                assert!(!(bound.admits)(bound.value - 1));
            }
        }
    }
}

/// APIs the crate claims not to reach for. `auth.rs` says the only I/O it performs is the
/// listener it was told to bind, and the verification record says it contains no logging macro;
/// both were prose until this case. A credential written to a log is the one leak the wire
/// cases cannot see, so the absence of a way to write one is checked against the source.
const FORBIDDEN_APIS: &[&str] = &[
    "std::fs",
    "fs::File",
    "File::open",
    "File::create",
    "OpenOptions",
    "TcpStream::connect",
    "UdpSocket",
    "std::process",
    "Command::new",
    "std::env",
    "env::var",
    "println!",
    "eprintln!",
    "print!",
    "eprint!",
    "dbg!",
    "io::stdout",
    "io::stderr",
    "tracing::",
];
// `log` and `tracing` cannot be reached at all: the crate declares no dependency and links
// nothing, which `tests/dependency_boundary.rs` asserts over the transitive closure. The list
// above is therefore the complete set of ways this crate could write a byte anywhere other
// than the socket it was handed.

#[test]
fn the_crate_reaches_no_io_beyond_the_listener_it_was_given() {
    assert!(
        SOURCES.len() >= 6,
        "the source list is not reading the crate"
    );
    for source in SOURCES {
        for api in FORBIDDEN_APIS {
            // The list itself is quoted inside this file, never inside the crate.
            assert!(
                !source.contains(api),
                "a source file of llm-gateway names {api}; the crate claims the only I/O it \
                 performs is the listener it was handed, and that a credential has nowhere to \
                 be logged to"
            );
        }
    }
    // Positive control: the scan really does look at the crate's text.
    assert!(
        SOURCES.iter().any(|source| source.contains("TcpListener")),
        "the scan is not reading the server source"
    );
}

/// `Authenticated` is a marker on the gateway's own request path, not a capability and not a
/// gate on the data. The crate documented the opposite conclusion from a true premise; this
/// pins what is actually true, so the sentence cannot drift back.
#[test]
fn proof_of_authentication_is_a_marker_on_the_request_path_not_a_capability() {
    // Any caller outside the crate mints one, because deciding who the owner is is the
    // embedding's job.
    let minted = Verdict::Owner.into_authenticated();
    assert!(minted.is_some());
    assert!(Verdict::Rejected.into_authenticated().is_none());

    // And the same identifier bytes are reachable with no proof at all, from an inventory the
    // caller built in the first place.
    let inventory = inventory();
    let route = inventory.route("code").expect("the alias is declared");
    assert_eq!(route.alias().as_str(), "code");
    assert_eq!(
        route.targets()[0].provenance.endpoint.as_str(),
        "remote-models"
    );

    // What does hold is the statement about the HTTP surface, which
    // `an_unauthenticated_inspection_is_refused_and_discloses_no_route` measures.
    let rendered = inventory.inspect(&minted.unwrap());
    assert!(rendered.contains("\"alias\":\"code\""));
}

/// The wire shape is written by explicit calls in `src/json.rs`, so a field cannot reach a
/// client by being added to a struct. This pins the exact key set that does reach one.
#[test]
fn an_inspection_response_carries_exactly_these_keys() {
    let gateway = Fixture::start(true);
    let body = gateway.get("/v1/routes", Some(OWNER_SECRET)).body;
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    let mut rest = body.as_str();
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let name = &after[..close];
        let tail = &after[close + 1..];
        if tail.starts_with(':') {
            keys.insert(name);
        }
        rest = tail;
    }
    let expected: BTreeSet<&str> = [
        "route_count",
        "routes",
        "route_id",
        "alias",
        "fallback_enabled",
        "target_count",
        "targets",
        "target_id",
        "position",
        "protocol",
        "provider",
        "account",
        "endpoint",
        "model",
        "binding_revision",
        "auth_kind",
        "billing_kind",
        "context_window",
        "max_output_tokens",
    ]
    .into_iter()
    .collect();
    assert_eq!(keys, expected, "body was {body}");
    // A digest nobody supplied is absent rather than empty, so it is not in the set above.
    assert!(!keys.contains("config_digest"));
}
