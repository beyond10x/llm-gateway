//! The listening surface: liveness, readiness, authenticated read-only inspection, and a
//! graceful stop.

use crate::{
    auth::{Authenticated, OwnerToken, OwnerVerifier},
    error::RefusalCode,
    inventory::RouteInventory,
    listing::{self, InstructionAnswer},
    metrics::{self, Disposition, Metrics, UsageRecord},
    relay::{self, Admitted, Call, ModelStatus, Relay, Wire},
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
/// The output clamp's count deadline. A timeout, not a size bound; its enforcement is measured
/// with a short configured value.
const DEFAULT_COUNT_TIMEOUT_SECONDS: u64 = 10;
/// The `retry-after` a `model-cold-start` refusal carries, in seconds, as llmgw (row W6). A hint
/// to the client, not a bound.
const COLD_START_RETRY_AFTER_SECONDS: u64 = 30;
/// The phrase a status line carries when no phrase is known. No status the crate can answer
/// with maps to it, which `every_status_the_crate_can_answer_carries_a_reason_phrase` checks.
const UNKNOWN_PHRASE: &str = "Unknown";
const ROUTES: &str = "/v1/routes";
const ROUTES_PREFIX: &str = "/v1/routes/";
/// The lifecycle routes' common prefix (`llm-gateway.gateway.LifecycleRoute`).
const MODELS_PREFIX: &str = "/v1/models/";
const WAKE_SUFFIX: &str = "/wake";
const STATUS_SUFFIX: &str = "/status";
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
    /// How long the output clamp's count may take, from the moment its connection to the target
    /// is open, to be written and answered. A count not answered by then fails, and the request
    /// is relayed unchanged; the target's own read timeout never holds it longer.
    pub count_timeout: Duration,
    /// Connections served at once. Beyond it a request is refused as unavailable.
    pub max_concurrent_requests: u64,
    /// Whether a model call takes the owner credential. [`IngressAuth::Bearer`] unless the
    /// embedding says otherwise.
    pub ingress_auth: IngressAuth,
}

/// Whether a request to a wire path of a gateway composed with a relay takes the owner
/// credential (story:ingress-auth-optional). Every other path authenticates either way, so the
/// owner-only surface (`/metrics`, `/v1/routes`) and an unknown path stay closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IngressAuth {
    /// The owner's bearer credential, as every other authenticated path. The default.
    #[default]
    Bearer,
    /// No credential: `authorization` is not read for a model call, so a missing, malformed or
    /// wrong one is no refusal. For a gateway behind something that authenticates already.
    None,
}

impl GatewayConfig {
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            max_head_bytes: DEFAULT_MAX_HEAD_BYTES,
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECONDS),
            count_timeout: Duration::from_secs(DEFAULT_COUNT_TIMEOUT_SECONDS),
            max_concurrent_requests: DEFAULT_MAX_CONCURRENT_REQUESTS,
            ingress_auth: IngressAuth::Bearer,
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
    /// The address bound, the origin `GET /` falls back to.
    local_addr: SocketAddr,
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
            local_addr,
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
            scrapers: Vec::new(),
        })
    }
}

/// Controls a running gateway. Dropping it without [`GatewayHandle::shutdown`] still stops the
/// listener, but only a shutdown reports what was in flight.
pub struct GatewayHandle {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    accept: Option<JoinHandle<()>>,
    /// The accept loops of the metrics listeners [`GatewayHandle::serve_metrics`] bound.
    scrapers: Vec<JoinHandle<()>>,
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

    /// Binds a metrics listener on `bind` and returns the address bound
    /// (`llm-gateway.telemetry`, docs/gateway.md "Scraping the counters"). It answers `GET` and
    /// `HEAD` on exactly `/metrics` with the counters, and every other method or path
    /// `404 path-unknown`, so no model route, probe or inspection path is reachable on it. It
    /// takes no owner credential: with `bearer` a scrape must present that token as
    /// `authorization: Bearer`, compared in constant time; without one it is open. It serves
    /// before [`GatewayHandle::mark_ready`] and while draining, and stops with the gateway.
    ///
    /// # Errors
    /// Returns the operating system's error when the address cannot be bound or cannot be put
    /// into non-blocking mode.
    pub fn serve_metrics(
        &mut self,
        bind: SocketAddr,
        bearer: Option<OwnerToken>,
    ) -> io::Result<SocketAddr> {
        let listener = TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        let local_addr = listener.local_addr()?;
        let scraper = Arc::new(Scraper {
            shared: Arc::clone(&self.shared),
            bearer: bearer.as_ref().map(OwnerToken::digest),
            in_flight: AtomicU64::new(0),
        });
        drop(bearer);
        self.scrapers
            .push(thread::spawn(move || scrape_loop(&listener, &scraper)));
        Ok(local_addr)
    }

    fn join_scrapers(&mut self) {
        for scraper in std::mem::take(&mut self.scrapers) {
            drop(scraper.join());
        }
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
        self.join_scrapers();
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
        self.join_scrapers();
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

/// One metrics listener: the gateway it scrapes, the digest of its scrape token, and its
/// connections being served.
struct Scraper {
    shared: Arc<Shared>,
    bearer: Option<[u8; 32]>,
    in_flight: AtomicU64,
}

/// Accepts scrapes while the gateway runs, then waits for those already accepted.
fn scrape_loop(listener: &TcpListener, scraper: &Arc<Scraper>) {
    let mut connections: Vec<JoinHandle<()>> = Vec::new();
    while scraper.shared.running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                scraper.in_flight.fetch_add(1, Ordering::SeqCst);
                let serving = Arc::clone(scraper);
                connections.push(thread::spawn(move || {
                    serve_scrape(stream, &serving);
                    serving.in_flight.fetch_sub(1, Ordering::SeqCst);
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

/// One connection to a metrics listener: its head, under the gateway's head bound and timeout,
/// then the counters or a refusal. Nothing but `/metrics` is decided.
fn serve_scrape(mut stream: TcpStream, scraper: &Scraper) {
    let config = &scraper.shared.config;
    let head_only;
    let outcome = if scraper.in_flight.load(Ordering::SeqCst) > config.max_concurrent_requests {
        head_only = false;
        Outcome::Refused(RefusalCode::Overloaded)
    } else {
        let deadline = Instant::now() + config.read_timeout;
        match read_head(&mut stream, config.max_head_bytes, deadline).and_then(|(head, _)| {
            Request::parse(&head, &|_, _, _| false)
                .map(|request| (request.head_only(), decide_scrape(&request, scraper)))
        }) {
            Ok((only, outcome)) => {
                head_only = only;
                outcome
            }
            Err(code) => {
                head_only = false;
                Outcome::Refused(code)
            }
        }
    };
    write_outcome(
        &mut stream,
        &outcome,
        head_only,
        Instant::now() + config.read_timeout,
    );
    drop(stream.shutdown(Shutdown::Both));
}

/// A scrape's answer: `GET` or `HEAD` on exactly `/metrics`, else `path-unknown`; then the
/// scrape token when one is set; then the counters, ready or not.
fn decide_scrape(request: &Request<'_>, scraper: &Scraper) -> Outcome {
    if request.path != METRICS || !request.is_read() {
        return Outcome::Refused(RefusalCode::PathUnknown);
    }
    if let Some(expected) = &scraper.bearer
        && let Err(code) = bearer(request).and_then(|token| {
            token
                .matches_digest(expected)
                .then_some(())
                .ok_or(RefusalCode::CredentialRejected)
        })
    {
        return Outcome::Refused(code);
    }
    if declares_body(request) {
        return Outcome::Refused(RefusalCode::BodyNotAllowed);
    }
    Outcome::Metrics(scraper.shared.metrics.render())
}

/// A decided response, before it is written.
enum Outcome {
    Served(String),
    /// An answer to a public route, with its content type (rows R5, R1).
    Public(&'static str, String),
    /// The counters, rendered as Prometheus text.
    Metrics(String),
    Probe(u16, String),
    Refused(RefusalCode),
    /// `method-not-allowed`, with the methods the path does take.
    NotAllowed(&'static str),
    /// A relay the head admitted; its body has not been read yet.
    Relay(Admitted),
    /// A lifecycle route's answer: `202` for a wake, `200` for a status.
    Lifecycle(u16, String),
}

/// One of the owner's two lifecycle routes per relayed model (story:model-wake-and-status).
#[derive(Clone, Copy)]
enum Lifecycle {
    /// `POST /v1/models/{alias}/wake`.
    Wake,
    /// `GET` or `HEAD /v1/models/{alias}/status`.
    Status,
}

impl Lifecycle {
    /// The route and its alias, when `path` is a lifecycle path: the bytes between
    /// [`MODELS_PREFIX`] and the route's last segment, compared as they are; an empty alias is
    /// none.
    fn of(path: &str) -> Option<(Self, &str)> {
        let rest = path.strip_prefix(MODELS_PREFIX)?;
        let (route, alias) = if let Some(alias) = rest.strip_suffix(WAKE_SUFFIX) {
            (Self::Wake, alias)
        } else {
            (Self::Status, rest.strip_suffix(STATUS_SUFFIX)?)
        };
        (!alias.is_empty()).then_some((route, alias))
    }

    const fn allow(self) -> &'static str {
        match self {
            Self::Wake => "POST",
            Self::Status => "GET, HEAD",
        }
    }
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
            Err(code) => (Decision::refused(code), Leftover(Vec::new())),
        };
    // A usage record's duration runs from the end of the head (llm-gateway.telemetry).
    let begun = Instant::now();
    let mut call = Call::default();
    let outcome = match (decision.outcome, &shared.relay) {
        (Outcome::Relay(admitted), Some(relay)) => {
            let limits = relay::Limits {
                line_bytes: shared.config.max_head_bytes,
                timeout,
                count_timeout: shared.config.count_timeout,
            };
            let leftover = leftover.into_bytes();
            match relay::serve(&mut stream, leftover, &admitted, relay, &limits, &mut call) {
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

/// Makes the usage record of an admitted request to a relayed wire (rows O1, O2; authenticated,
/// or needing no credential under `IngressAuth::None`): `wire`
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
        None => (
            Disposition::Relayed,
            call.target_status.unwrap_or_default(),
            call.response_bytes,
        ),
        Some(RefusalCode::UpstreamFailed) => (
            Disposition::UpstreamFailed,
            RefusalCode::UpstreamFailed.status(),
            0,
        ),
        Some(code) => (Disposition::Refused, code.status(), 0),
    };
    relay.observe(&UsageRecord {
        outcome: metrics::Outcome::of(refusal, call.timed_out),
        model: call.model,
        wire,
        target_status: call.target_status,
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
        profile: call.profile,
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
            // An interrupted read is retried: a signal is not the client closing.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Ok(0) | Err(_) => break,
            Ok(read) => discarded += u64::try_from(read).unwrap_or(u64::MAX),
        }
    }
    drop(stream.shutdown(Shutdown::Both));
}

/// Reads exactly the request head, bounded in size and in time, and returns it with whatever
/// arrived after it. Only a relay reads further; every other path ignores those bytes.
///
/// The head carries the owner's credential when the client presents one, so its bytes are only
/// ever held in a [`HeadBuffer`], which each read writes into directly: an allocation it leaves
/// for a larger one is overwritten before it is freed, and the last one when the buffer, or the
/// [`Head`] it becomes, is dropped, on every path out of here, refusals included. What arrived
/// after the head is a [`Leftover`], overwritten when dropped too: a pipelining client's next
/// request, and its credential, may be in it.
fn read_head(
    stream: &mut impl HeadStream,
    limit: usize,
    deadline: Instant,
) -> Result<(Head, Leftover), RefusalCode> {
    let mut buffer = HeadBuffer(Vec::new());
    let mut filled = 0;
    loop {
        if let Some(end) = find_terminator(&buffer.0[..filled]) {
            if std::str::from_utf8(&buffer.0[..end]).is_err() {
                return Err(RefusalCode::RequestMalformed);
            }
            let leftover = Leftover(buffer.0[end + 4..filled].to_vec());
            #[cfg(test)]
            head_ledger::taken(&leftover.0);
            return Ok((Head { buffer, end }, leftover));
        }
        // A byte `Request::parse` refuses wherever it stands is refused as soon as it arrives:
        // a head holding one may never send CRLFCRLF, and is malformed at any length.
        if holds_stray_line_byte(&buffer.0[..filled]) {
            return Err(RefusalCode::RequestMalformed);
        }
        // Never read past the bound, so an oversized head is refused rather than buffered.
        let room = limit.saturating_sub(filled).min(READ_CHUNK);
        if room == 0 {
            return Err(RefusalCode::RequestTooLarge);
        }
        // Each read waits only for what is left of the deadline, so the head as a whole is bounded.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.wait_at_most(left).is_err() {
            return Err(RefusalCode::RequestMalformed);
        }
        match stream.read(buffer.room(filled, room, limit)) {
            // An interrupted read is retried: a signal is not the peer stopping.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // A peer that stops early, times out or resets never delivered a head.
            Ok(0) | Err(_) => return Err(RefusalCode::RequestMalformed),
            Ok(read) => filled += read,
        }
    }
}

/// Where a head is read from: a client's connection, or a scripted one in a test.
trait HeadStream: Read {
    /// Lets the next read wait at most `left`.
    fn wait_at_most(&mut self, left: Duration) -> io::Result<()>;
}

impl HeadStream for TcpStream {
    fn wait_at_most(&mut self, left: Duration) -> io::Result<()> {
        self.set_read_timeout(Some(left))
    }
}

/// A client's request head as it is read. Its bytes are in one allocation at a time, which a
/// read writes into directly, so no copy of them is left on the stack. When a read needs more
/// room they move to an allocation twice the size, at most the head bound, and the one they
/// leave is overwritten first; the last is overwritten when the buffer is dropped.
struct HeadBuffer(Vec<u8>);

impl HeadBuffer {
    /// The `wanted` bytes after the first `filled`, which the next read writes into. Without
    /// room for them the bytes move first, to no more than `limit` unless `filled + wanted` is.
    fn room(&mut self, filled: usize, wanted: usize, limit: usize) -> &mut [u8] {
        let needed = filled + wanted;
        if self.0.len() < needed {
            let mut grown = vec![0_u8; self.0.len().saturating_mul(2).min(limit).max(needed)];
            #[cfg(test)]
            head_ledger::taken(&grown);
            grown[..filled].copy_from_slice(&self.0[..filled]);
            wipe(&mut std::mem::replace(&mut self.0, grown));
        }
        &mut self.0[filled..needed]
    }
}

impl Drop for HeadBuffer {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// A head [`read_head`] returned: its bytes up to the CRLFCRLF, which are UTF-8, in the buffer
/// they were read into, and overwritten with it when the head is dropped.
struct Head {
    buffer: HeadBuffer,
    end: usize,
}

impl std::ops::Deref for Head {
    type Target = str;

    fn deref(&self) -> &str {
        // `read_head` returns no head that is not UTF-8, so this never answers the empty text.
        std::str::from_utf8(&self.buffer.0[..self.end]).unwrap_or_default()
    }
}

/// The bytes [`read_head`] read past the head: the start of a body, or a pipelining client's
/// next request, its credential included. Overwritten when dropped, on every path that does
/// not relay; a relay takes them as they are ([`Leftover::into_bytes`]).
struct Leftover(Vec<u8>);

impl Leftover {
    /// The bytes, for the relay, which reads the body on from them.
    fn into_bytes(mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Leftover {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Overwrites every byte of `bytes`' allocation, its spare capacity included, so it is freed
/// holding nothing. Best effort without a `zeroize` dependency, as `OwnerToken` does: the black
/// box keeps the writes observable.
fn wipe(bytes: &mut Vec<u8>) {
    bytes.fill(0);
    bytes.resize(bytes.capacity(), 0);
    std::hint::black_box(&*bytes);
    #[cfg(test)]
    head_ledger::released(bytes);
}

/// Each head allocation this thread took and let go, so that a test can check every one was
/// overwritten before it was freed and none was freed behind the buffer's back.
#[cfg(test)]
mod head_ledger {
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum Entry {
        /// An allocation's address and size, as the buffer took it.
        Taken(usize, usize),
        /// An allocation's address and size as it was let go, and whether every byte read zero.
        Released(usize, usize, bool),
    }

    thread_local! {
        static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn taken(bytes: &Vec<u8>) {
        if bytes.capacity() > 0 {
            let entry = Entry::Taken(bytes.as_ptr().addr(), bytes.capacity());
            ENTRIES.with_borrow_mut(|entries| entries.push(entry));
        }
    }

    pub(super) fn released(bytes: &Vec<u8>) {
        if bytes.capacity() > 0 {
            let zeroed = bytes.len() == bytes.capacity() && bytes.iter().all(|byte| *byte == 0);
            let entry = Entry::Released(bytes.as_ptr().addr(), bytes.capacity(), zeroed);
            ENTRIES.with_borrow_mut(|entries| entries.push(entry));
        }
    }

    /// This thread's entries since the last call, oldest first.
    pub(super) fn drain() -> Vec<Entry> {
        ENTRIES.with_borrow_mut(std::mem::take)
    }
}

/// The head reader over every way a read can end, each with the owner's credential already in
/// the buffer. What these prove is the most safe Rust can observe: every allocation that held
/// the head's bytes was taken by the buffer itself, at most the head bound, and read zero in
/// full, spare capacity included, as it was let go, on every path. That the freed memory stays
/// zero is not observable here; the writes are best effort, as `OwnerToken`'s are.
#[cfg(test)]
mod head_tests {
    use super::{HeadStream, READ_CHUNK, head_ledger, head_ledger::Entry, read_head};
    use crate::error::RefusalCode;
    use std::{
        collections::{BTreeSet, VecDeque},
        io::{self, Read},
        time::{Duration, Instant},
    };

    const CREDENTIAL: &str = "authorization: Bearer sk-head-credential-0123456789abcdef\r\n";

    /// What a scripted connection answers its reads with, in order.
    type Reads = Vec<io::Result<Vec<u8>>>;

    /// A connection that answers each read with the next of `reads`, cut to the room offered,
    /// and refuses a wait once `waits` are spent.
    struct Scripted {
        reads: VecDeque<io::Result<Vec<u8>>>,
        waits: Option<usize>,
    }

    impl Scripted {
        fn new(reads: Reads) -> Self {
            Self {
                reads: reads.into(),
                waits: None,
            }
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match self.reads.pop_front() {
                None => Ok(0),
                Some(Err(error)) => Err(error),
                Some(Ok(mut bytes)) => {
                    let given = bytes.len().min(buffer.len());
                    buffer[..given].copy_from_slice(&bytes[..given]);
                    if given < bytes.len() {
                        self.reads.push_front(Ok(bytes.split_off(given)));
                    }
                    Ok(given)
                }
            }
        }
    }

    impl HeadStream for Scripted {
        fn wait_at_most(&mut self, _left: Duration) -> io::Result<()> {
            match &mut self.waits {
                Some(0) => Err(io::Error::from(io::ErrorKind::TimedOut)),
                Some(left) => {
                    *left -= 1;
                    Ok(())
                }
                None => Ok(()),
            }
        }
    }

    /// A head with the credential on its second line and one padding header, `length` bytes in
    /// all with its CRLFCRLF.
    fn head(length: usize) -> Vec<u8> {
        let start = format!("POST /v1/messages HTTP/1.1\r\n{CREDENTIAL}x-pad: ");
        let mut head = start.into_bytes();
        head.resize(length - 4, b'a');
        head.extend_from_slice(b"\r\n\r\n");
        head
    }

    /// `bytes` in reads of at most `size` bytes.
    fn chunks(bytes: &[u8], size: usize) -> Reads {
        bytes.chunks(size).map(|chunk| Ok(chunk.to_vec())).collect()
    }

    /// Every allocation taken was let go once, read zero in full, and nothing was let go that
    /// was not taken; returns how many were taken.
    fn every_allocation_was_overwritten(case: &str, limit: usize) -> usize {
        let mut live = BTreeSet::new();
        let mut taken = 0;
        for entry in head_ledger::drain() {
            match entry {
                Entry::Taken(address, size) => {
                    assert!(
                        size <= limit,
                        "{case}: an allocation of {size} past the bound"
                    );
                    assert!(live.insert((address, size)), "{case}: taken twice");
                    taken += 1;
                }
                Entry::Released(address, size, zeroed) => {
                    assert!(
                        live.remove(&(address, size)),
                        "{case}: let go an allocation the buffer never took, so the one it did \
                         take was freed by a reallocation without being overwritten"
                    );
                    assert!(
                        zeroed,
                        "{case}: an allocation was let go holding head bytes"
                    );
                }
            }
        }
        assert!(
            live.is_empty(),
            "{case}: {} allocation(s) holding head bytes were freed without being overwritten",
            live.len()
        );
        assert!(
            taken > 0,
            "{case}: the head's bytes were held in no buffer the ledger saw"
        );
        taken
    }

    #[test]
    fn a_head_is_overwritten_in_every_allocation_it_used_when_it_is_answered() {
        let limit = 8192;
        let sent = head(3000);
        let mut reads = vec![Err(io::Error::from(io::ErrorKind::Interrupted))];
        reads.extend(chunks(&sent, 700));
        reads.push(Ok(b"{}".to_vec()));
        let mut stream = Scripted::new(reads);
        let (head, leftover) =
            read_head(&mut stream, limit, Instant::now() + Duration::from_secs(10))
                .expect("a whole head is read");
        assert_eq!(head.as_bytes(), &sent[..sent.len() - 4]);
        assert!(
            leftover.0.is_empty() || leftover.0 == b"{}",
            "{:?}",
            leftover.0
        );
        drop((head, leftover));
        let taken = every_allocation_was_overwritten("answered", limit);
        assert!(
            taken >= 2,
            "a head of 3000 bytes read {READ_CHUNK} at a time moves at least once"
        );
    }

    /// What this proves: under a bound that doubling from `READ_CHUNK` does not reach exactly,
    /// the last move takes the bound itself, so no allocation that holds the head is larger
    /// than the bound (`every_allocation_was_overwritten` refuses any past `limit`), and each
    /// is still overwritten as it is let go.
    #[test]
    fn a_head_never_moves_into_an_allocation_past_its_bound() {
        let limit = 3000;
        let sent = head(2900);
        let mut stream = Scripted::new(chunks(&sent, 700));
        let (head, leftover) =
            read_head(&mut stream, limit, Instant::now() + Duration::from_secs(10))
                .expect("a whole head is read");
        assert_eq!(head.as_bytes(), &sent[..sent.len() - 4]);
        assert!(leftover.0.is_empty(), "{:?}", leftover.0);
        drop((head, leftover));
        assert_eq!(
            every_allocation_was_overwritten("doubling past the bound", limit),
            3,
            "1024 bytes, then 2048, then the bound of 3000 rather than 4096"
        );
    }

    /// What this proves: when what arrived after a head is a pipelining client's next request,
    /// its credential included, those bytes are held in an allocation of their own that reads
    /// zero in full, spare capacity included, as it is let go, as each of the head's own does.
    #[test]
    fn the_bytes_past_a_head_are_overwritten_when_they_are_let_go() {
        let limit = 8192;
        let first = head(600);
        let next = head(400);
        let mut stream = Scripted::new(vec![Ok([&first[..], &next[..]].concat())]);
        let (head, leftover) =
            read_head(&mut stream, limit, Instant::now() + Duration::from_secs(10))
                .expect("the first head is read");
        assert_eq!(head.as_bytes(), &first[..first.len() - 4]);
        assert_eq!(leftover.0, next, "the next request is what is left");
        drop((head, leftover));
        assert_eq!(
            every_allocation_was_overwritten("pipelined", limit),
            2,
            "the head's buffer and the bytes past it"
        );
    }

    #[test]
    fn a_refused_head_is_overwritten_in_every_allocation_it_used_on_every_path() {
        let limit = 2048;
        let start = format!("POST /v1/messages HTTP/1.1\r\n{CREDENTIAL}").into_bytes();
        let with = |rest: &[u8]| [&start[..], rest].concat();
        let stray = with(b"x-note: a\nb");
        let oversized = with(&vec![b'a'; limit]);
        let not_utf8 = with(b"x-note: \xff\r\n\r\n");
        let cases: Vec<(&str, Reads, Option<usize>, RefusalCode)> = vec![
            (
                "a stray line byte",
                chunks(&stray, 1000),
                None,
                RefusalCode::RequestMalformed,
            ),
            (
                "past the bound",
                chunks(&oversized, READ_CHUNK),
                None,
                RefusalCode::RequestTooLarge,
            ),
            (
                "not UTF-8",
                chunks(&not_utf8, 1000),
                None,
                RefusalCode::RequestMalformed,
            ),
            (
                "the peer stops",
                chunks(&start, 1000),
                None,
                RefusalCode::RequestMalformed,
            ),
            (
                "the peer resets",
                vec![
                    Ok(start.clone()),
                    Err(io::Error::from(io::ErrorKind::ConnectionReset)),
                ],
                None,
                RefusalCode::RequestMalformed,
            ),
            (
                "the deadline passes",
                vec![Ok(start.clone()), Ok(b"x-more: a".to_vec())],
                Some(1),
                RefusalCode::RequestMalformed,
            ),
        ];
        for (case, reads, waits, code) in cases {
            let mut stream = Scripted::new(reads);
            stream.waits = waits;
            let refused = read_head(&mut stream, limit, Instant::now() + Duration::from_secs(10))
                .map(|_| ())
                .expect_err(case);
            assert_eq!(refused, code, "{case}");
            every_allocation_was_overwritten(case, limit);
        }
    }
}

fn find_terminator(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Whether a partial head holds a NUL, an LF that no CR precedes, or a CR that something other
/// than an LF follows. A CR at the very end is not decided yet: its LF may be in the next read.
fn holds_stray_line_byte(buffer: &[u8]) -> bool {
    buffer.iter().enumerate().any(|(at, byte)| match byte {
        0 => true,
        b'\n' => at == 0 || buffer[at - 1] != b'\r',
        b'\r' => buffer.get(at + 1).is_some_and(|next| *next != b'\n'),
        _ => false,
    })
}

/// A decided response together with whether its body is written.
struct Decision {
    outcome: Outcome,
    head_only: bool,
    /// Whether the connection is closed gracefully after a refusal: a request to a relayed
    /// wire may still be sending a body the gateway never read.
    linger: bool,
    /// The wire of an admitted request to a relayed wire path (authenticated, or needing no
    /// credential under `IngressAuth::None`), which makes a usage
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
    /// Each field's value without its optional whitespace (SP and HTAB, RFC 9110 section 5.6.3).
    headers: Vec<(String, &'a str)>,
}

impl<'a> Request<'a> {
    /// `ignores_repeat(method, path, name)` names the headers a request never reads: a repeated
    /// one there is ignored rather than refused.
    fn parse(
        head: &'a str,
        ignores_repeat: &dyn Fn(&str, &str, &str) -> bool,
    ) -> Result<Self, RefusalCode> {
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
                if ignores_repeat(method, path, &name) {
                    continue;
                }
                // A repeated credential is ambiguous, and an ambiguous credential is refused.
                return Err(if name == "authorization" {
                    RefusalCode::CredentialMalformed
                } else {
                    RefusalCode::RequestMalformed
                });
            }
            headers.push((name, value.trim_matches([' ', '\t'])));
        }
        Ok(Self {
            method,
            path,
            http_1_0: version == "HTTP/1.0",
            headers,
        })
    }

    /// The value of `name` with every Unicode whitespace at its edges trimmed.
    fn header(&self, name: &str) -> Option<&'a str> {
        self.field(name).map(str::trim)
    }

    /// The value of `name` as the client sent it, without its optional whitespace only: a
    /// value checked byte for byte, such as the caller's session.
    fn field(&self, name: &str) -> Option<&'a str> {
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

/// Whether `path` is a model call whose credential is never read: a wire path of a relaying
/// gateway whose `ingress_auth` is `None` (story:ingress-auth-optional).
fn reads_no_credential(shared: &Shared, path: &str) -> bool {
    shared.config.ingress_auth == IngressAuth::None
        && shared.relay.is_some()
        && Wire::from_path(path).is_some()
}

/// Whether a probe or a public route never reads `name`, so a repeat of it is no ambiguity and
/// the request is still answered: a probe reads no header, and a public route reads only its
/// body framing, `host` and `user-agent` (story:probes-before-head-checks).
fn answers_any_repeat(method: &str, path: &str, name: &str) -> bool {
    if !matches!(method, "GET" | "HEAD") {
        return false;
    }
    match path {
        "/health" | "/ready" => true,
        listing::MODELS | listing::INSTRUCTIONS => !matches!(
            name,
            "content-length" | "transfer-encoding" | "host" | "user-agent"
        ),
        _ => false,
    }
}

fn dispatch(head: &str, shared: &Shared) -> Decision {
    let ignores_repeat = |method: &str, path: &str, name: &str| {
        answers_any_repeat(method, path, name)
            || (name == "authorization" && reads_no_credential(shared, path))
    };
    let request = match Request::parse(head, &ignores_repeat) {
        Ok(request) => request,
        Err(code) => return Decision::refused(code),
    };
    let head_only = request.head_only();
    // A wake is a `POST` too, and may still be sending a body it is refused for.
    let linger = shared.relay.is_some()
        && (Wire::from_path(request.path).is_some() || Lifecycle::of(request.path).is_some());
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
    // The probes: two literal paths with two literal methods, matched before load is shed.
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
    // The public routes: two more literal paths with the same two methods, answered without a
    // credential. Any other method goes on to authentication, so it cannot enumerate anything.
    if request.is_read() && matches!(request.path, listing::MODELS | listing::INSTRUCTIONS) {
        return (public(request, shared), None);
    }
    // A model call under `IngressAuth::None` reads no credential (story:ingress-auth-optional);
    // every other path still authenticates below, so nothing else opens.
    if reads_no_credential(shared, request.path)
        && let Some(wire) = Wire::from_path(request.path)
    {
        return (admit(request, shared, wire), Some(wire));
    }
    // Everything else establishes the authenticated context before decoding anything further.
    let owner = match authenticate(request, shared) {
        Ok(owner) => owner,
        Err(code) => return (Outcome::Refused(code), None),
    };
    if let (Some(wire), Some(_)) = (Wire::from_path(request.path), &shared.relay) {
        return (admit(request, shared, wire), Some(wire));
    }
    if let (Some((route, alias)), Some(relay)) = (Lifecycle::of(request.path), &shared.relay) {
        return (lifecycle(request, shared, relay, route, alias), None);
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

/// `GET /v1/models` (row R5) and `GET /` (row R1), under inspection's body and readiness rules.
/// Both read the relayed models alone: never the inventory, the target source or a credential.
fn public(request: &Request<'_>, shared: &Shared) -> Outcome {
    if declares_body(request) {
        return Outcome::Refused(RefusalCode::BodyNotAllowed);
    }
    if !serves_inspection(shared) {
        return Outcome::Refused(RefusalCode::Unavailable);
    }
    if request.path == listing::MODELS {
        let models = shared.relay.iter().flat_map(Relay::models);
        let translates = shared.relay.as_ref().is_some_and(Relay::translates);
        return Outcome::Public(JSON, listing::models(models, translates));
    }
    let answer = InstructionAnswer::choose(request.header("user-agent"));
    let origin = listing::origin(request.header("host"), &shared.local_addr);
    shared.metrics.count_instruction_view();
    // Each client is profiled where its requests reach a model (story:client-profiles-translated).
    Outcome::Public(
        answer.content_type(),
        answer.render(shared.relay.as_ref(), &origin),
    )
}

/// A lifecycle route (`llm-gateway.gateway.LifecycleRoute`), after authentication: its method,
/// a declared body and readiness are decided before the target source is asked anything.
fn lifecycle(
    request: &Request<'_>,
    shared: &Shared,
    relay: &Relay,
    route: Lifecycle,
    alias: &str,
) -> Outcome {
    let allowed = match route {
        Lifecycle::Wake => request.method == "POST",
        Lifecycle::Status => request.is_read(),
    };
    if !allowed {
        return Outcome::NotAllowed(route.allow());
    }
    if declares_body(request) {
        return Outcome::Refused(RefusalCode::BodyNotAllowed);
    }
    if !serves_inspection(shared) {
        return Outcome::Refused(RefusalCode::Unavailable);
    }
    let answered = match route {
        Lifecycle::Wake => relay.wake(alias).map(|status| (202, status)),
        Lifecycle::Status => relay.status(alias).map(|status| (200, status)),
    };
    match answered {
        Ok((code, status)) => Outcome::Lifecycle(code, lifecycle_body(alias, &status)),
        Err(code) => Outcome::Refused(code),
    }
}

/// `{"model","phase","sub_phase"?,"profile"?}`: a value the source did not report, or reported
/// empty, is absent, never `""`.
fn lifecycle_body(alias: &str, status: &ModelStatus) -> String {
    let mut writer = crate::json::Writer::new();
    writer.begin_object();
    writer.key("model");
    writer.string(alias);
    writer.key("phase");
    writer.string(status.phase.code());
    for (key, value) in [
        ("sub_phase", &status.sub_phase),
        ("profile", &status.profile),
    ] {
        if let Some(value) = value.as_deref().filter(|value| !value.is_empty()) {
            writer.key(key);
            writer.string(value);
        }
    }
    writer.end_object();
    writer.finish()
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
            anthropic_beta: request.header("anthropic-beta").map(str::to_owned),
            session: request
                .field(relay::CALLER_SESSION_HEADER)
                .map(str::to_owned),
        }),
        Err(code) => Outcome::Refused(code),
    }
}

/// The bearer token `authorization` presents: `credential-absent` without one,
/// `credential-malformed` when it is not one bearer token.
fn bearer(request: &Request<'_>) -> Result<OwnerToken, RefusalCode> {
    let presented = request
        .header("authorization")
        .ok_or(RefusalCode::CredentialAbsent)?;
    let (scheme, material) = presented
        .split_once(' ')
        .ok_or(RefusalCode::CredentialMalformed)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return Err(RefusalCode::CredentialMalformed);
    }
    OwnerToken::new(material.trim().as_bytes().to_vec())
        .map_err(|_| RefusalCode::CredentialMalformed)
}

fn authenticate(request: &Request<'_>, shared: &Shared) -> Result<Authenticated, RefusalCode> {
    let token = bearer(request)?;
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
        Outcome::Served(body) | Outcome::Metrics(body) | Outcome::Public(_, body) => {
            (200, body.clone(), None)
        }
        Outcome::Probe(status, body) | Outcome::Lifecycle(status, body) => {
            (*status, body.clone(), None)
        }
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
        Outcome::Public(content_type, _) => content_type,
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
        202 => "Accepted",
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
        assert!(
            Request::parse("POST /v1/messages HTTP/1.1\r\nx-note: a", &|_, _, _| false).is_ok()
        );
        for head in [
            "POST /v1/messages HTTP/1.1\r\nx-note: a\ntransfer-encoding: chunked",
            "POST /v1/messages HTTP/1.1\r\nx-note: a\rb",
            "POST /v1/messages HTTP/1.1\r\nx-note: a\0b",
            "POST /v1/messages\n HTTP/1.1\r\nx-note: a",
            "POST /v1/messages HTTP/1.1\r\nx-n\note: a",
        ] {
            assert_eq!(
                Request::parse(head, &|_, _, _| false).err(),
                Some(RefusalCode::RequestMalformed),
                "{head:?}"
            );
        }
    }

    /// The same bytes are refused in a head that has not arrived whole, so one that never sends
    /// CRLFCRLF is not left to the head bound or the timeout; a final CR is not decided yet.
    #[test]
    fn a_partial_head_with_a_stray_line_byte_is_caught_before_it_ends() {
        for partial in [
            &b"GET /health HTTP/1.1\nx: a"[..],
            b"\nGET /health HTTP/1.1",
            b"GET /health HTTP/1.1\r\nx: a\rb",
            b"GET /health HTTP/1.1\r\nx: a\0",
        ] {
            assert!(super::holds_stray_line_byte(partial), "{partial:?}");
        }
        for partial in [
            &b"GET /health HTTP/1.1\r\nx: a\r\n"[..],
            b"GET /health HTTP/1.1\r\nx: a\r",
            b"",
        ] {
            assert!(!super::holds_stray_line_byte(partial), "{partial:?}");
        }
    }

    /// The reason phrase is the one property of a refusal the `refusal_codes!` macro does not
    /// generate, because it belongs to the status rather than to the code. A variant added with
    /// a status outside the match below would answer `HTTP/1.1 <status> Unknown` with every
    /// other check green, so the map is checked against every status the crate can answer with.
    #[test]
    fn every_status_the_crate_can_answer_carries_a_reason_phrase() {
        let mut statuses: Vec<u16> = RefusalCode::ALL.iter().map(|code| code.status()).collect();
        // The two the probe and inspection paths answer with outside a refusal, and a wake's.
        statuses.push(200);
        statuses.push(503);
        statuses.push(202);
        for status in statuses {
            assert_ne!(
                reason_phrase(status),
                UNKNOWN_PHRASE,
                "status {status} reaches a client with no reason phrase"
            );
        }
    }
}
