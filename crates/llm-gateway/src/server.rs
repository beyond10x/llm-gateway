//! The listening surface: liveness, readiness, authenticated read-only inspection, and a
//! graceful stop.

use crate::{
    auth::{Authenticated, OwnerToken, OwnerVerifier},
    error::RefusalCode,
    inventory::RouteInventory,
    metrics::{Disposition, Metrics, UsageRecord},
    relay::{self, Admitted, Call, Relay, Wire},
};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const ACCEPT_POLL: Duration = Duration::from_millis(2);
/// Internal read buffer size. Not a bound on anything a caller can observe.
const READ_CHUNK: usize = 1024;
const DEFAULT_MAX_HEAD_BYTES: usize = 8192;
const DEFAULT_MAX_CONCURRENT_REQUESTS: u64 = 64;
/// A timeout, not a size bound. Its enforcement is measured with a short configured value.
const DEFAULT_READ_TIMEOUT_SECONDS: u64 = 10;
/// The `retry-after` a `model-cold-start` refusal carries, in seconds, as llmgw (row W6). A hint
/// to the client, not a bound.
const COLD_START_RETRY_AFTER_SECONDS: u64 = 30;
/// The phrase a status line carries when no phrase is known. No status the crate can answer
/// with maps to it, which `every_status_the_crate_can_answer_carries_a_reason_phrase` checks.
const UNKNOWN_PHRASE: &str = "Unknown";
const ROUTES: &str = "/v1/routes";
const ROUTES_PREFIX: &str = "/v1/routes/";
/// The owner's counters as Prometheus text (row R4).
const METRICS: &str = "/metrics";
const JSON: &str = "application/json";
/// The Prometheus text exposition format, version 0.0.4.
const PROMETHEUS_TEXT: &str = "text/plain; version=0.0.4; charset=utf-8";

/// How the gateway listens. It holds no credential: the verifier is a separate argument, so a
/// configuration value can be logged or written to disk without leaking anything.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind: SocketAddr,
    /// Bound on the request head. A larger head is refused, never buffered.
    pub max_head_bytes: usize,
    /// How long one connection may take to deliver its head.
    pub read_timeout: Duration,
    /// Connections served at once. Beyond it a request is refused as unavailable.
    pub max_concurrent_requests: u64,
}

impl GatewayConfig {
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            max_head_bytes: DEFAULT_MAX_HEAD_BYTES,
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECONDS),
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
        }
    }
}

/// What a graceful stop observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Connections accepted over the gateway's life.
    pub accepted: u64,
    /// Connections whose handling finished. After a graceful stop this equals `accepted`.
    pub completed: u64,
    /// Connections still being handled when the stop was signalled.
    pub in_flight_at_signal: u64,
}

struct Shared {
    config: GatewayConfig,
    verifier: Arc<dyn OwnerVerifier>,
    inventory: RouteInventory,
    relay: Option<Relay>,
    /// The relay's counters, or the gateway's own when it relays nothing.
    metrics: Arc<Metrics>,
    running: AtomicBool,
    ready: AtomicBool,
    draining: AtomicBool,
    accepted: AtomicU64,
    completed: AtomicU64,
    in_flight: AtomicU64,
}

/// Composes and starts the gateway.
pub struct Gateway;

impl Gateway {
    /// Binds the listener and starts accepting. The gateway begins **not ready**: call
    /// [`GatewayHandle::mark_ready`] once the embedding has finished composing.
    ///
    /// # Errors
    /// Returns the operating system's error when the address cannot be bound or cannot be put
    /// into non-blocking mode.
    pub fn bind(
        config: GatewayConfig,
        verifier: Arc<dyn OwnerVerifier>,
        inventory: RouteInventory,
    ) -> io::Result<GatewayHandle> {
        Self::compose(config, verifier, inventory, None)
    }

    /// Binds like [`Gateway::bind`], and also relays the three model-call wires to the targets
    /// `relay` hands out (story:wire-relay). Without a relay the wire paths are not served.
    ///
    /// # Errors
    /// As [`Gateway::bind`].
    pub fn bind_with_relay(
        config: GatewayConfig,
        verifier: Arc<dyn OwnerVerifier>,
        inventory: RouteInventory,
        relay: Relay,
    ) -> io::Result<GatewayHandle> {
        Self::compose(config, verifier, inventory, Some(relay))
    }

    fn compose(
        config: GatewayConfig,
        verifier: Arc<dyn OwnerVerifier>,
        inventory: RouteInventory,
        relay: Option<Relay>,
    ) -> io::Result<GatewayHandle> {
        let listener = TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let metrics = relay.as_ref().map_or_else(
            || Arc::new(Metrics::default()),
            |relay| Arc::clone(relay.metrics()),
        );
        let shared = Arc::new(Shared {
            config,
            verifier,
            inventory,
            relay,
            metrics,
            running: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            accepted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
        });
        let accepting = Arc::clone(&shared);
        let accept = thread::spawn(move || accept_loop(&listener, &accepting));
        Ok(GatewayHandle {
            shared,
            local_addr,
            accept: Some(accept),
        })
    }
}

/// Controls a running gateway. Dropping it without [`GatewayHandle::shutdown`] still stops the
/// listener, but only a shutdown reports what was in flight.
pub struct GatewayHandle {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    accept: Option<JoinHandle<()>>,
}

impl GatewayHandle {
    /// The address actually bound, which is what a caller needs after binding port zero.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Declares the gateway ready to serve inspection. It does **not** undo a drain: a drain is
    /// one way for the life of the handle, so an embedding that re-marks readiness periodically
    /// cannot put a draining gateway back into rotation.
    pub fn mark_ready(&self) {
        self.shared.ready.store(true, Ordering::SeqCst);
    }

    /// Reports unready while continuing to serve. This is the drain: a load balancer removes the
    /// gateway from rotation, and the requests already in flight or that still arrive finish
    /// normally. It cannot be undone; the next step is [`GatewayHandle::shutdown`].
    pub fn begin_drain(&self) {
        self.shared.draining.store(true, Ordering::SeqCst);
    }

    /// Whether inspection is currently served.
    pub fn is_ready(&self) -> bool {
        is_ready(&self.shared)
    }

    /// Stops accepting, lets every connection already accepted finish, and reports what
    /// happened. This is the graceful stop.
    pub fn shutdown(mut self) -> ShutdownReport {
        let in_flight_at_signal = self.shared.in_flight.load(Ordering::SeqCst);
        // Readiness is reported false at once, so a load balancer stops sending. Inspection
        // keeps being served until the last accepted connection has finished, because a
        // connection this report counts as completed must not have been answered with a
        // refusal. `ready` is therefore cleared after the join, never before it.
        self.shared.draining.store(true, Ordering::SeqCst);
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            drop(accept.join());
        }
        self.shared.ready.store(false, Ordering::SeqCst);
        ShutdownReport {
            accepted: self.shared.accepted.load(Ordering::SeqCst),
            completed: self.shared.completed.load(Ordering::SeqCst),
            in_flight_at_signal,
        }
    }
}

impl Drop for GatewayHandle {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            drop(accept.join());
        }
    }
}

impl std::fmt::Debug for GatewayHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewayHandle")
            .field("local_addr", &self.local_addr)
            .field("ready", &self.is_ready())
            .finish_non_exhaustive()
    }
}

/// What a readiness probe reports. Draining is reported unready so a load balancer removes the
/// gateway from rotation.
fn is_ready(shared: &Shared) -> bool {
    serves_inspection(shared) && !shared.draining.load(Ordering::SeqCst)
}

/// Whether inspection is actually served. A drain must keep serving what still arrives — that
/// is the point of draining — so this deliberately ignores the draining flag.
fn serves_inspection(shared: &Shared) -> bool {
    shared.ready.load(Ordering::SeqCst)
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while shared.running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                shared.accepted.fetch_add(1, Ordering::SeqCst);
                shared.in_flight.fetch_add(1, Ordering::SeqCst);
                let serving = Arc::clone(shared);
                connections.push(thread::spawn(move || {
                    serve(stream, &serving);
                    serving.in_flight.fetch_sub(1, Ordering::SeqCst);
                    serving.completed.fetch_add(1, Ordering::SeqCst);
                }));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                connections.retain(|handle| !handle.is_finished());
                thread::sleep(ACCEPT_POLL);
            }
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
    for connection in connections {
        drop(connection.join());
    }
}

/// A decided response, before it is written.
enum Outcome {
    Served(String),
    /// The counters, rendered as Prometheus text.
    Metrics(String),
    Probe(u16, String),
    Refused(RefusalCode),
    /// `method-not-allowed`, with the methods the path does take.
    NotAllowed(&'static str),
    /// A relay the head admitted; its body has not been read yet.
    Relay(Admitted),
}

fn serve(mut stream: TcpStream, shared: &Shared) {
    // One deadline for the whole head and one for the whole response, never a timeout per
    // read or write: a peer that trickles a byte at a time would otherwise hold the connection,
    // and with it a graceful stop, for as long as it likes.
    let timeout = shared.config.read_timeout;
    let head_deadline = Instant::now() + timeout;
    // Load is shed inside `decide`, after the liveness and readiness paths have been matched:
    // a liveness probe that fails under load restarts the process, which turns shedding into a
    // restart loop. Reading a bounded head under a timeout is the cheapest way to know whether
    // a request is a probe, so it happens first.
    let (decision, leftover) =
        match read_head(&mut stream, shared.config.max_head_bytes, head_deadline) {
            Ok((head, leftover)) => (dispatch(&head, shared), leftover),
            Err(code) => (Decision::refused(code), Vec::new()),
        };
    // A usage record's duration runs from the end of the head (llm-gateway.telemetry).
    let begun = Instant::now();
    let mut call = Call::default();
    let outcome = match (decision.outcome, &shared.relay) {
        (Outcome::Relay(admitted), Some(relay)) => {
            let limits = relay::Limits {
                line_bytes: shared.config.max_head_bytes,
                timeout,
            };
            match relay::serve(&mut stream, leftover, admitted, relay, &limits, &mut call) {
                Ok(()) => {
                    record(shared, decision.recorded, call, None, begun);
                    drop(stream.shutdown(Shutdown::Both));
                    return;
                }
                Err(code) => Outcome::Refused(code),
            }
        }
        (Outcome::Relay(_), None) => Outcome::Refused(RefusalCode::PathUnknown),
        (outcome, _) => outcome,
    };
    write_outcome(
        &mut stream,
        &outcome,
        decision.head_only,
        Instant::now() + timeout,
    );
    let refusal = match outcome {
        Outcome::Refused(code) => Some(code),
        Outcome::NotAllowed(_) => Some(RefusalCode::MethodNotAllowed),
        _ => None,
    };
    if let Some(code) = refusal {
        // Written before the connection is closed, so a client that has read its answer to
        // the end can count on the record having been made.
        record(shared, decision.recorded, call, Some(code), begun);
    }
    if decision.linger {
        linger(&mut stream, Instant::now() + timeout);
    } else {
        drop(stream.shutdown(Shutdown::Both));
    }
}

/// Makes the usage record of an authenticated request to a relayed wire (rows O1, O2): `wire`
/// is `None` for every other request, which makes no record. `refusal` is what the client was
/// answered instead of a relayed answer.
fn record(
    shared: &Shared,
    wire: Option<Wire>,
    call: Call,
    refusal: Option<RefusalCode>,
    begun: Instant,
) {
    let (Some(wire), Some(relay)) = (wire, &shared.relay) else {
        return;
    };
    let (disposition, status, response_bytes) = match refusal {
        None => (Disposition::Relayed, call.status, call.response_bytes),
        Some(RefusalCode::UpstreamFailed) => (
            Disposition::UpstreamFailed,
            RefusalCode::UpstreamFailed.status(),
            0,
        ),
        Some(code) => (Disposition::Refused, code.status(), 0),
    };
    relay.observe(&UsageRecord {
        model: call.model,
        wire,
        disposition,
        refusal,
        status,
        reported_model: None,
        input_tokens: None,
        output_tokens: None,
        cached_input_tokens: None,
        cache_creation_input_tokens: None,
        reasoning_output_tokens: None,
        response_bytes,
        duration_ms: u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX),
    });
}

/// Closes a relay connection that was refused gracefully: the answer is already written, so
/// the write side is closed and what the client still sends is read and discarded until it
/// closes, the deadline passes or the body bound is reached. Closing with unread bytes would
/// reset the connection, and a reset can destroy the refusal before the client reads it.
fn linger(stream: &mut TcpStream, deadline: Instant) {
    drop(stream.shutdown(Shutdown::Write));
    let mut chunk = [0_u8; READ_CHUNK];
    let mut discarded: u64 = 0;
    while discarded <= relay::MAX_REQUEST_BODY_BYTES {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => discarded += u64::try_from(read).unwrap_or(u64::MAX),
        }
    }
    drop(stream.shutdown(Shutdown::Both));
}

/// Reads exactly the request head, bounded in size and in time, and returns it with whatever
/// arrived after it. Only a relay reads further; every other path ignores those bytes.
fn read_head(
    stream: &mut TcpStream,
    limit: usize,
    deadline: Instant,
) -> Result<(String, Vec<u8>), RefusalCode> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK];
    loop {
        if let Some(end) = find_terminator(&buffer) {
            let leftover = buffer[end + 4..].to_vec();
            return String::from_utf8(buffer[..end].to_vec())
                .map(|head| (head, leftover))
                .map_err(|_| RefusalCode::RequestMalformed);
        }
        // Never read past the bound, so an oversized head is refused rather than buffered.
        let room = limit.saturating_sub(buffer.len()).min(READ_CHUNK);
        if room == 0 {
            return Err(RefusalCode::RequestTooLarge);
        }
        // Each read waits only for what is left of the deadline, so the head as a whole is bounded.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return Err(RefusalCode::RequestMalformed);
        }
        match stream.read(&mut chunk[..room]) {
            // A peer that stops early, times out or resets never delivered a head.
            Ok(0) | Err(_) => return Err(RefusalCode::RequestMalformed),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
        }
    }
}

fn find_terminator(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// A decided response together with whether its body is written.
struct Decision {
    outcome: Outcome,
    head_only: bool,
    /// Whether the connection is closed gracefully after a refusal: a request to a relayed
    /// wire may still be sending a body the gateway never read.
    linger: bool,
    /// The wire of an authenticated request to a relayed wire path, which makes a usage
    /// record whatever answers it.
    recorded: Option<Wire>,
}

impl Decision {
    fn refused(code: RefusalCode) -> Self {
        Self {
            outcome: Outcome::Refused(code),
            head_only: false,
            linger: false,
            recorded: None,
        }
    }
}

/// One parsed request head. Nothing beyond the head is ever read.
struct Request<'a> {
    method: &'a str,
    path: &'a str,
    /// `HTTP/1.0`, which takes neither a chunked answer nor an interim `100 Continue`.
    http_1_0: bool,
    headers: Vec<(String, &'a str)>,
}

impl<'a> Request<'a> {
    fn parse(head: &'a str) -> Result<Self, RefusalCode> {
        // RFC 9110 section 5.5: CR, LF and NUL are never part of a field. Once the head is split
        // at its CRLFs, any that remain would let another reader see a line this one does not.
        if head
            .split("\r\n")
            .any(|line| line.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)))
        {
            return Err(RefusalCode::RequestMalformed);
        }
        let mut lines = head.split("\r\n");
        let start = lines.next().ok_or(RefusalCode::RequestMalformed)?;
        let mut parts = start.split(' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(RefusalCode::RequestMalformed);
        };
        if !version.starts_with("HTTP/1.") || method.is_empty() {
            return Err(RefusalCode::RequestMalformed);
        }
        // A query string is discarded: this milestone decodes no request parameter.
        let path = target.split('?').next().unwrap_or(target);
        if !path.starts_with('/') {
            return Err(RefusalCode::RequestMalformed);
        }
        let mut headers: Vec<(String, &'a str)> = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                // Obsolete line folding is refused rather than reassembled.
                return Err(RefusalCode::RequestMalformed);
            }
            let (name, value) = line.split_once(':').ok_or(RefusalCode::RequestMalformed)?;
            // RFC 9112 section 5.1: no whitespace is allowed between a field name and its colon,
            // and a server must reject such a request with 400.
            if name != name.trim_end() || name.is_empty() || name.contains(' ') {
                return Err(RefusalCode::RequestMalformed);
            }
            let name = name.to_ascii_lowercase();
            if headers.iter().any(|(seen, _)| *seen == name) {
                // A repeated credential is ambiguous, and an ambiguous credential is refused.
                return Err(if name == "authorization" {
                    RefusalCode::CredentialMalformed
                } else {
                    RefusalCode::RequestMalformed
                });
            }
            headers.push((name, value.trim()));
        }
        Ok(Self {
            method,
            path,
            http_1_0: version == "HTTP/1.0",
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&'a str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| *value)
    }

    fn is_read(&self) -> bool {
        self.method == "GET" || self.method == "HEAD"
    }

    fn head_only(&self) -> bool {
        self.method == "HEAD"
    }
}

fn dispatch(head: &str, shared: &Shared) -> Decision {
    let request = match Request::parse(head) {
        Ok(request) => request,
        Err(code) => return Decision::refused(code),
    };
    let head_only = request.head_only();
    let linger = shared.relay.is_some() && Wire::from_path(request.path).is_some();
    let (outcome, recorded) = decide(&request, shared);
    Decision {
        outcome,
        head_only,
        linger,
        recorded,
    }
}

/// The outcome, and the wire when the request is an authenticated one to a relayed wire path.
fn decide(request: &Request<'_>, shared: &Shared) -> (Outcome, Option<Wire>) {
    // The unauthenticated surface is a closed set of two literal paths with two literal methods.
    if request.is_read() {
        match request.path {
            "/health" => return (Outcome::Probe(200, status_body("live")), None),
            "/ready" => {
                let probe = if is_ready(shared) {
                    Outcome::Probe(200, status_body("ready"))
                } else {
                    Outcome::Probe(503, status_body("unready"))
                };
                return (probe, None);
            }
            _ => {}
        }
    }
    // Shedding is not decoding: an overloaded gateway refuses before spending a verification,
    // because shedding has to be cheaper than the work it sheds.
    if shared.in_flight.load(Ordering::SeqCst) > shared.config.max_concurrent_requests {
        return (Outcome::Refused(RefusalCode::Overloaded), None);
    }
    // Everything else establishes the authenticated context before decoding anything further.
    let owner = match authenticate(request, shared) {
        Ok(owner) => owner,
        Err(code) => return (Outcome::Refused(code), None),
    };
    if let (Some(wire), Some(_)) = (Wire::from_path(request.path), &shared.relay) {
        return (admit(request, shared, wire), Some(wire));
    }
    if request.path == METRICS {
        return (scrape(request, shared), None);
    }
    (inspect(request, shared, &owner), None)
}

/// `GET /metrics` (row R4): the owner's counters, under inspection's method, body and
/// readiness rules.
fn scrape(request: &Request<'_>, shared: &Shared) -> Outcome {
    if !request.is_read() {
        return Outcome::NotAllowed("GET, HEAD");
    }
    if declares_body(request) {
        return Outcome::Refused(RefusalCode::BodyNotAllowed);
    }
    if !serves_inspection(shared) {
        return Outcome::Refused(RefusalCode::Unavailable);
    }
    Outcome::Metrics(shared.metrics.render())
}

fn declares_body(request: &Request<'_>) -> bool {
    request.header("transfer-encoding").is_some()
        || request
            .header("content-length")
            .is_some_and(|value| value != "0")
}

/// Decides from the head alone whether a relay may read its body: the method, readiness, and
/// the body framing and its declared size (rows R6-R8 and W1).
fn admit(request: &Request<'_>, shared: &Shared, wire: Wire) -> Outcome {
    if request.method != "POST" {
        return Outcome::NotAllowed("POST");
    }
    if !serves_inspection(shared) {
        return Outcome::Refused(RefusalCode::Unavailable);
    }
    match relay::framing(
        request.header("content-length"),
        request.header("transfer-encoding"),
    ) {
        Ok(framing) => Outcome::Relay(Admitted {
            wire,
            framing,
            expects_continue: !request.http_1_0
                && request
                    .header("expect")
                    .is_some_and(|value| value.eq_ignore_ascii_case("100-continue")),
            chunked_answer: !request.http_1_0,
        }),
        Err(code) => Outcome::Refused(code),
    }
}

fn authenticate(request: &Request<'_>, shared: &Shared) -> Result<Authenticated, RefusalCode> {
    let presented = request
        .header("authorization")
        .ok_or(RefusalCode::CredentialAbsent)?;
    let (scheme, material) = presented
        .split_once(' ')
        .ok_or(RefusalCode::CredentialMalformed)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err(RefusalCode::CredentialMalformed);
    }
    let token = OwnerToken::new(material.trim().as_bytes().to_vec())
        .map_err(|_| RefusalCode::CredentialMalformed)?;
    shared
        .verifier
        .verify(&token)
        .into_authenticated()
        .ok_or(RefusalCode::CredentialRejected)
}

fn inspect(request: &Request<'_>, shared: &Shared, owner: &Authenticated) -> Outcome {
    if !request.is_read() {
        return Outcome::NotAllowed("GET, HEAD");
    }
    if declares_body(request) {
        return Outcome::Refused(RefusalCode::BodyNotAllowed);
    }
    if !serves_inspection(shared) {
        return Outcome::Refused(RefusalCode::Unavailable);
    }
    if request.path == ROUTES {
        return Outcome::Served(shared.inventory.inspect(owner));
    }
    if let Some(alias) = request.path.strip_prefix(ROUTES_PREFIX) {
        return shared
            .inventory
            .inspect_alias(owner, alias)
            .map_or(Outcome::Refused(RefusalCode::RouteUnknown), Outcome::Served);
    }
    Outcome::Refused(RefusalCode::PathUnknown)
}

fn status_body(status: &str) -> String {
    let mut writer = crate::json::Writer::new();
    writer.begin_object();
    writer.key("status");
    writer.string(status);
    writer.end_object();
    writer.finish()
}

fn write_outcome(stream: &mut TcpStream, outcome: &Outcome, head_only: bool, deadline: Instant) {
    let refused = |code: RefusalCode| (code.status(), refusal_body(code), None);
    let (status, body, allow) = match outcome {
        Outcome::Served(body) | Outcome::Metrics(body) => (200, body.clone(), None),
        Outcome::Probe(status, body) => (*status, body.clone(), None),
        Outcome::Refused(code) => refused(*code),
        Outcome::NotAllowed(allow) => {
            let code = RefusalCode::MethodNotAllowed;
            (code.status(), refusal_body(code), Some(*allow))
        }
        // `serve` relays an admitted request before anything is written; one that reaches
        // here had no relay to go to.
        Outcome::Relay(_) => refused(RefusalCode::PathUnknown),
    };
    let content_type = match outcome {
        Outcome::Metrics(_) => PROMETHEUS_TEXT,
        _ => JSON,
    };
    let mut head = String::new();
    head.push_str("HTTP/1.1 ");
    head.push_str(&status.to_string());
    head.push(' ');
    head.push_str(reason_phrase(status));
    head.push_str("\r\ncontent-type: ");
    head.push_str(content_type);
    head.push_str("\r\n");
    head.push_str("content-length: ");
    head.push_str(&body.len().to_string());
    head.push_str("\r\ncache-control: no-store\r\nconnection: close\r\n");
    if status == 401 {
        head.push_str("www-authenticate: Bearer\r\n");
    }
    if matches!(outcome, Outcome::Refused(RefusalCode::ModelColdStart)) {
        head.push_str("retry-after: ");
        head.push_str(&COLD_START_RETRY_AFTER_SECONDS.to_string());
        head.push_str("\r\n");
    }
    if let Some(allow) = allow {
        head.push_str("allow: ");
        head.push_str(allow);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    if !write_by(stream, head.as_bytes(), deadline) {
        return;
    }
    if !head_only {
        write_by(stream, body.as_bytes(), deadline);
    }
}

/// Writes all of `bytes` before `deadline`, or gives up. A peer that stops reading cannot hold
/// the connection past the deadline, so it cannot hold a graceful stop either.
pub(crate) fn write_by(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> bool {
    while !bytes.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_write_timeout(Some(left)).is_err() {
            return false;
        }
        match stream.write(bytes) {
            Ok(0) => return false,
            Ok(written) => bytes = &bytes[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return false,
        }
    }
    true
}

fn refusal_body(code: RefusalCode) -> String {
    let mut writer = crate::json::Writer::new();
    writer.begin_object();
    writer.key("error");
    writer.begin_object();
    writer.key("code");
    writer.string(code.wire());
    writer.key("message");
    writer.string(code.reason());
    writer.end_object();
    writer.end_object();
    writer.finish()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => UNKNOWN_PHRASE,
    }
}

/// The reason phrase of a relayed status. A target may answer with any status; one this
/// gateway has no phrase for is relayed with an empty one, which HTTP/1.1 allows.
pub(crate) fn relayed_phrase(status: u16) -> &'static str {
    match status {
        201 => "Created",
        202 => "Accepted",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => match reason_phrase(status) {
            UNKNOWN_PHRASE => "",
            phrase => phrase,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Request, UNKNOWN_PHRASE, reason_phrase};
    use crate::error::RefusalCode;

    /// RFC 9110 section 5.5: a CR, LF or NUL left inside a line once the head is split at its
    /// CRLFs is refused wherever it sits, so no other reader can see a line this one does not.
    #[test]
    fn a_cr_lf_or_nul_inside_a_head_line_is_malformed() {
        assert!(Request::parse("POST /v1/messages HTTP/1.1\r\nx-note: a").is_ok());
        for head in [
            "POST /v1/messages HTTP/1.1\r\nx-note: a\ntransfer-encoding: chunked",
            "POST /v1/messages HTTP/1.1\r\nx-note: a\rb",
            "POST /v1/messages HTTP/1.1\r\nx-note: a\0b",
            "POST /v1/messages\n HTTP/1.1\r\nx-note: a",
            "POST /v1/messages HTTP/1.1\r\nx-n\note: a",
        ] {
            assert_eq!(
                Request::parse(head).err(),
                Some(RefusalCode::RequestMalformed),
                "{head:?}"
            );
        }
    }

    /// The reason phrase is the one property of a refusal the `refusal_codes!` macro does not
    /// generate, because it belongs to the status rather than to the code. A variant added with
    /// a status outside the match below would answer `HTTP/1.1 <status> Unknown` with every
    /// other check green, so the map is checked against every status the crate can answer with.
    #[test]
    fn every_status_the_crate_can_answer_carries_a_reason_phrase() {
        let mut statuses: Vec<u16> = RefusalCode::ALL.iter().map(|code| code.status()).collect();
        // The two the probe and inspection paths answer with outside a refusal.
        statuses.push(200);
        statuses.push(503);
        for status in statuses {
            assert_ne!(
                reason_phrase(status),
                UNKNOWN_PHRASE,
                "status {status} reaches a client with no reason phrase"
            );
        }
    }
}
