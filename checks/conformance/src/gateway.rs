//! Gateway conformance observations over a loopback socket.
//!
//! This module composes the real `b10x-llm-gateway` — an `OwnerToken`, a `SharedSecretVerifier`
//! and a `RouteInventory` — binds it to `127.0.0.1:0`, sends the authored raw HTTP/1.1 requests
//! and reports what the gateway wrote back. It parses nothing the gateway did not write, decides
//! no refusal, reads no suite and branches on no scenario name. The only I/O is the loopback
//! connection to the listener this module bound.

use std::{
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use ess_conformance::target::TargetError;
use llm_gateway::{
    AuthKind, BillingKind, Gateway, GatewayConfig, GatewayHandle, InventoryError, Label,
    LabelError, OwnerToken, RouteInventory, RouteSummary, SharedSecretVerifier, TargetLimits,
    TargetProvenance, TargetSummary, TokenError, VerifierError,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::target::Observed;

/// Every view name this domain answers through `query_view`.
pub const VIEWS: &[&str] = &[
    "llm-gateway.gateway.LastExchange",
    "llm-gateway.gateway.LastRelay",
];

const MAX_PROGRAM_BYTES: usize = 64 * 1024;
const MAX_STEPS: usize = 64;
const MAX_PAD_BYTES: usize = 64 * 1024;
/// Twice the crate's route bound, so a fixture can step past it and no further.
const MAX_GENERATED_ROUTES: usize = 8192;
/// How long the client waits for the gateway's answer. Far above any authored read timeout.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Observe one command of this domain, or `None` when the command belongs to another.
pub fn observe(command: &str, input: &Value) -> Option<Result<Observed, TargetError>> {
    match command {
        "llm-gateway.gateway.Exchange" => Some(exercise(input)),
        "llm-gateway.gateway.Relay" => Some(relay::exercise(input)),
        _ => None,
    }
}

fn unavailable(error: impl std::fmt::Display) -> TargetError {
    TargetError::unavailable("gateway observation", error.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    program_json: String,
}

fn exercise(input: &Value) -> Result<Observed, TargetError> {
    let request: Exchange = serde_json::from_value(input.clone()).map_err(unavailable)?;
    Ok(Observed {
        facts: run(&request.program_json),
        view: "llm-gateway.gateway.LastExchange",
        event: "llm-gateway.gateway.Exchanged",
        field: "valid_program",
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    secret: String,
    #[serde(default)]
    max_head_bytes: Option<usize>,
    #[serde(default)]
    read_timeout_ms: Option<u64>,
    #[serde(default)]
    max_concurrent_requests: Option<u64>,
    #[serde(default)]
    inventory: Option<InventoryInput>,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryInput {
    #[serde(default)]
    config_digest: Option<String>,
    routes: Vec<RouteInput>,
    /// This many further routes, appended after `routes`: route `g<i>`, alias `g<i>`, one
    /// target `g<i>-t` at position 0, for `i` from 0. A count no authored list could fit in
    /// the program bound.
    #[serde(default)]
    generated_routes: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteInput {
    route_id: String,
    alias: String,
    fallback_enabled: bool,
    targets: Vec<TargetInput>,
}

/// A target; provenance fields default to the crate test suite's fixture (`tests/gateway.rs`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetInput {
    target_id: String,
    position: usize,
    #[serde(default)]
    auth_kind: Option<String>,
    #[serde(default)]
    billing_kind: Option<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    MarkReady,
    Drain,
    IsReady,
    /// Write the whole request, then read the answer to its end.
    Send {
        raw: String,
        /// A header of this many pad bytes, inserted before the head's final empty line.
        #[serde(default)]
        pad_bytes: Option<usize>,
    },
    /// Write a head that never ends, then read whatever the gateway answers.
    SendPartial {
        raw: String,
    },
    Shutdown,
}

fn label_code(error: LabelError) -> &'static str {
    match error {
        LabelError::Length => "length",
        LabelError::NotPrintableAscii => "not-printable-ascii",
    }
}

fn inventory_code(error: InventoryError) -> &'static str {
    match error {
        InventoryError::RouteWithoutTarget => "route-without-target",
        InventoryError::TooManyTargets => "too-many-targets",
        InventoryError::TooManyRoutes => "too-many-routes",
        InventoryError::NonContiguousPositions => "non-contiguous-positions",
        InventoryError::DuplicateTarget => "duplicate-target",
        InventoryError::DuplicateAlias => "duplicate-alias",
        InventoryError::MalformedDigest => "malformed-digest",
    }
}

fn token_code(error: TokenError) -> &'static str {
    match error {
        TokenError::Empty => "empty",
        TokenError::TooLarge => "too-large",
        TokenError::NotPrintableAscii => "not-printable-ascii",
    }
}

fn verifier_code(error: VerifierError) -> &'static str {
    match error {
        VerifierError::SecretTooShort => "secret-too-short",
    }
}

fn label(value: &str) -> Result<Label, String> {
    Label::new(value).map_err(|error| format!("label:{}", label_code(error)))
}

fn target(input: &TargetInput) -> Result<TargetSummary, String> {
    Ok(TargetSummary {
        target_id: label(&input.target_id)?,
        position: input.position,
        provenance: TargetProvenance {
            protocol: label("chat-completions")?,
            provider: label("my-lab")?,
            account: label("remote")?,
            endpoint: label("remote-models")?,
            model: label("large")?,
            binding_revision: label("rev-1")?,
        },
        auth_kind: match input.auth_kind.as_deref() {
            None | Some("bearer") => AuthKind::Bearer,
            Some("anonymous") => AuthKind::Anonymous,
            Some("api-key") => AuthKind::ApiKey,
            Some(_) => return Err("fixture:unknown-auth-kind".to_owned()),
        },
        billing_kind: match input.billing_kind.as_deref() {
            None | Some("metered") => BillingKind::Metered,
            Some("subscription") => BillingKind::Subscription,
            Some("self-hosted") => BillingKind::SelfHosted,
            Some(_) => return Err("fixture:unknown-billing-kind".to_owned()),
        },
        limits: TargetLimits {
            context_window: input.context_window,
            max_output_tokens: input.max_output_tokens,
        },
    })
}

/// The crate test suite's inventory: one route whose second target reports no limits.
fn default_inventory() -> InventoryInput {
    let at = |target_id: &str, position, limits: Option<(u64, u64)>| TargetInput {
        target_id: target_id.to_owned(),
        position,
        auth_kind: None,
        billing_kind: None,
        context_window: limits.map(|(context, _)| context),
        max_output_tokens: limits.map(|(_, output)| output),
    };
    InventoryInput {
        config_digest: None,
        generated_routes: 0,
        routes: vec![RouteInput {
            route_id: "coding".to_owned(),
            alias: "code".to_owned(),
            fallback_enabled: false,
            targets: vec![
                at("coding-primary", 0, Some((4096, 1024))),
                at("coding-secondary", 1, None),
            ],
        }],
    }
}

fn inventory(input: &InventoryInput) -> Result<RouteInventory, String> {
    let inventory_error = |error| format!("inventory:{}", inventory_code(error));
    let mut routes = Vec::new();
    for route in &input.routes {
        let targets = route.targets.iter().map(target).collect::<Result<_, _>>()?;
        routes.push(
            RouteSummary::new(
                label(&route.route_id)?,
                label(&route.alias)?,
                route.fallback_enabled,
                targets,
            )
            .map_err(inventory_error)?,
        );
    }
    for index in 0..input.generated_routes {
        let name = format!("g{index}");
        let only = TargetInput {
            target_id: format!("{name}-t"),
            position: 0,
            auth_kind: None,
            billing_kind: None,
            context_window: None,
            max_output_tokens: None,
        };
        routes.push(
            RouteSummary::new(label(&name)?, label(&name)?, false, vec![target(&only)?])
                .map_err(inventory_error)?,
        );
    }
    let snapshot = RouteInventory::new(routes).map_err(inventory_error)?;
    match &input.config_digest {
        None => Ok(snapshot),
        Some(digest) => snapshot.with_config_digest(digest).map_err(inventory_error),
    }
}

fn compose(program: &Program) -> Result<GatewayHandle, String> {
    let token = OwnerToken::new(program.secret.as_bytes().to_vec())
        .map_err(|error| format!("token:{}", token_code(error)))?;
    let verifier = SharedSecretVerifier::new(token)
        .map_err(|error| format!("verifier:{}", verifier_code(error)))?;
    let snapshot = inventory(program.inventory.as_ref().unwrap_or(&default_inventory()))?;
    let bind = "127.0.0.1:0"
        .parse()
        .map_err(|_| "fixture:bind-address".to_owned())?;
    let mut config = GatewayConfig::new(bind);
    if let Some(bytes) = program.max_head_bytes {
        config.max_head_bytes = bytes;
    }
    if let Some(ms) = program.read_timeout_ms {
        config.read_timeout = Duration::from_millis(ms);
    }
    if let Some(count) = program.max_concurrent_requests {
        config.max_concurrent_requests = count;
    }
    Gateway::bind(config, Arc::new(verifier), snapshot).map_err(|_| "bind:refused".to_owned())
}

/// What one exchange wrote back, split once at the end of its head.
struct Answer {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    body: String,
    raw: Vec<u8>,
}

fn exchange(addr: SocketAddr, request: &[u8], complete: bool) -> Answer {
    let mut raw = Vec::new();
    if let Ok(mut stream) = TcpStream::connect(addr) {
        let _timeout = stream.set_read_timeout(Some(CLIENT_TIMEOUT));
        if stream.write_all(request).is_ok() && stream.flush().is_ok() {
            if complete {
                let _half = stream.shutdown(Shutdown::Write);
            }
            // A reset after the answer is not a lost answer: keep what arrived.
            let _read = stream.read_to_end(&mut raw);
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|status| status.parse().ok());
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    Answer {
        status,
        headers,
        body: body.to_owned(),
        raw,
    }
}

impl Answer {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map_or("absent", |(_, value)| value.as_str())
    }

    /// The `answer_headers` fact: the three headers every answer the gateway writes carries.
    fn framing(&self) -> String {
        format!(
            "content-type={} cache-control={} connection={}",
            self.header("content-type"),
            self.header("cache-control"),
            self.header("connection")
        )
    }

    fn summary(&self) -> String {
        let Some(status) = self.status else {
            return "no-response".to_owned();
        };
        let word = if self.body.is_empty() {
            "no-body".to_owned()
        } else {
            match serde_json::from_str::<Value>(&self.body) {
                Ok(value) => value["error"]["code"]
                    .as_str()
                    .or_else(|| value["status"].as_str())
                    .unwrap_or("inspected")
                    .to_owned(),
                Err(_) => "unparsed".to_owned(),
            }
        };
        format!("{status} {word}")
    }
}

fn padded(raw: &str, pad: Option<usize>) -> Option<String> {
    let Some(pad) = pad else {
        return Some(raw.to_owned());
    };
    if pad > MAX_PAD_BYTES {
        return None;
    }
    let head = raw.strip_suffix("\r\n")?;
    Some(format!("{head}x-pad: {}\r\n\r\n", "a".repeat(pad)))
}

fn run(program_json: &str) -> Value {
    let mut facts = json!({
        "valid_program": false, "error_code": null, "results": [], "bodies": [],
        "headers": [], "answer_headers": [], "secret_on_wire": null, "ready": null,
        "shutdown": null
    });
    if program_json.len() > MAX_PROGRAM_BYTES {
        return facts;
    }
    let Ok(program) = serde_json::from_str::<Program>(program_json) else {
        return facts;
    };
    if program.steps.len() > MAX_STEPS
        || program
            .inventory
            .as_ref()
            .is_some_and(|inventory| inventory.generated_routes > MAX_GENERATED_ROUTES)
    {
        return facts;
    }
    facts["valid_program"] = json!(true);
    let mut handle = match compose(&program) {
        Ok(handle) => Some(handle),
        Err(code) => {
            facts["error_code"] = json!(code);
            return facts;
        }
    };
    let addr = handle.as_ref().map(GatewayHandle::local_addr);
    let secret = program.secret.as_bytes();
    let mut on_wire = false;
    let (mut results, mut bodies, mut headers) = (Vec::new(), Vec::new(), Vec::new());
    let mut answer_headers = Vec::new();
    for step in &program.steps {
        let answer = match step {
            Step::Send { raw, pad_bytes } => padded(raw, *pad_bytes)
                .zip(addr)
                .map(|(raw, addr)| exchange(addr, raw.as_bytes(), true)),
            Step::SendPartial { raw } => addr.map(|addr| exchange(addr, raw.as_bytes(), false)),
            Step::MarkReady | Step::Drain | Step::IsReady | Step::Shutdown => None,
        };
        let result = match (step, &answer) {
            (_, Some(answer)) => {
                on_wire |= !secret.is_empty()
                    && answer
                        .raw
                        .windows(secret.len())
                        .any(|window| window == secret);
                bodies.push(answer.body.clone());
                headers.push(format!(
                    "www-authenticate={} allow={}",
                    answer.header("www-authenticate"),
                    answer.header("allow")
                ));
                answer_headers.push(answer.framing());
                answer.summary()
            }
            (Step::MarkReady, None) => handle
                .as_ref()
                .map_or("stopped", |live| {
                    live.mark_ready();
                    "ok"
                })
                .to_owned(),
            (Step::Drain, None) => handle
                .as_ref()
                .map_or("stopped", |live| {
                    live.begin_drain();
                    "ok"
                })
                .to_owned(),
            (Step::IsReady, None) => handle.as_ref().map_or("stopped".to_owned(), |live| {
                format!("ready={}", live.is_ready())
            }),
            (Step::Shutdown, None) => handle.take().map_or("stopped".to_owned(), |live| {
                let report = live.shutdown();
                let summary = format!(
                    "accepted={} completed={}",
                    report.accepted, report.completed
                );
                facts["shutdown"] = json!(summary);
                summary
            }),
            (Step::Send { .. } | Step::SendPartial { .. }, None) => "no-response".to_owned(),
        };
        results.push(result);
    }
    facts["results"] = json!(results);
    facts["bodies"] = json!(bodies);
    facts["headers"] = json!(headers);
    facts["answer_headers"] = json!(answer_headers);
    facts["secret_on_wire"] = json!(on_wire);
    facts["ready"] = json!(handle.as_ref().map(GatewayHandle::is_ready));
    if let Some(live) = handle {
        live.shutdown();
    }
    facts
}

/// The `Relay` command: the real gateway composed with relayed models and a target source whose
/// targets are loopback fixture pods in this process. Nothing leaves the machine.
mod relay {
    use super::{Observed, label, token_code, unavailable, verifier_code};
    use ess_conformance::target::TargetError;
    use llm_gateway::{
        Disposition, Gateway, GatewayConfig, GatewayHandle, OwnerToken, Relay, RelayError,
        RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory, SharedSecretVerifier,
        TargetBearer, TargetRefusal, ToolCalling, UsageRecord, UsageRecords, Wire,
    };
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::{
        collections::{BTreeMap, VecDeque},
        io::{self, Read, Write},
        net::{Shutdown, SocketAddr, TcpListener, TcpStream},
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    const MAX_PROGRAM_BYTES: usize = 64 * 1024;
    const MAX_STEPS: usize = 64;
    const MAX_PODS: usize = 16;
    const MAX_ANSWERS: usize = 64;
    /// Twice the relay's body bound, so a fixture can step past it and no further.
    const MAX_PAD_BYTES: usize = 64 * 1024 * 1024;
    /// A recorded upstream body longer than this is written as its length.
    const RECORDED_BODY_BYTES: usize = 4096;
    /// How long a pod waits for the client to receive one chunk before it releases the next.
    const CHUNK_HOLD: Duration = Duration::from_secs(2);
    /// How long either side waits for the other. Far above any relay this program makes.
    const WAIT: Duration = Duration::from_secs(10);

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Command {
        program_json: String,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Program {
        secret: String,
        models: Vec<ModelInput>,
        pods: Vec<Vec<AnswerInput>>,
        /// The source's first `cold_starts` acquisitions report the model still starting past
        /// the hold budget (row W6).
        #[serde(default)]
        cold_starts: usize,
        steps: Vec<Step>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ModelInput {
        alias: String,
        upstream_model: String,
        wires: Vec<String>,
        /// The key every target handed out for this model carries (row B8).
        #[serde(default)]
        bearer: Option<String>,
        /// `parsed` or `absent`, default `absent`, as the deployment document's `tool_calling`.
        #[serde(default)]
        tool_calling: Option<String>,
    }

    /// One scripted answer: a reply, or a transport failure.
    #[derive(Deserialize, Clone)]
    #[serde(deny_unknown_fields)]
    struct AnswerInput {
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        content_type: Option<String>,
        #[serde(default)]
        chunks: Vec<String>,
        #[serde(default)]
        framing: Option<String>,
        #[serde(default)]
        transport: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
    enum Step {
        MarkReady,
        /// A `GET /metrics` (row R4), with the owner's credential unless `credential` is false.
        Scrape {
            #[serde(default)]
            credential: Option<bool>,
        },
        Request {
            path: String,
            body: String,
            #[serde(default)]
            method: Option<String>,
            #[serde(default)]
            credential: Option<bool>,
            #[serde(default)]
            pad_to_bytes: Option<usize>,
            #[serde(default)]
            declare_bytes: Option<u64>,
        },
    }

    impl AnswerInput {
        fn valid(&self) -> bool {
            match (&self.transport, self.status) {
                (Some(transport), None) => {
                    matches!(transport.as_str(), "refused" | "closed")
                        && self.chunks.is_empty()
                        && self.content_type.is_none()
                        && self.framing.is_none()
                }
                (None, Some(status)) => {
                    (200..=599).contains(&status)
                        && matches!(self.framing.as_deref(), None | Some("length" | "chunked"))
                }
                _ => false,
            }
        }
    }

    /// How far the client has read the answer to the current request.
    #[derive(Default)]
    struct Progress {
        received: Mutex<usize>,
        arrived: Condvar,
        /// A pod answered this request with a reply.
        replied: Mutex<Option<(u16, Vec<u8>)>>,
        /// A pod had to release a chunk before the client received the one before it.
        held: AtomicBool,
    }

    impl Progress {
        fn set_received(&self, count: usize) {
            if let Ok(mut received) = self.received.lock() {
                *received = count;
            }
            self.arrived.notify_all();
        }

        /// Waits until the client has received `count` body bytes; false if it did not in time.
        fn wait_for(&self, count: usize) -> bool {
            let deadline = Instant::now() + CHUNK_HOLD;
            let Ok(mut received) = self.received.lock() else {
                return false;
            };
            while *received < count {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return false;
                }
                match self.arrived.wait_timeout(received, left) {
                    Ok((next, _)) => received = next,
                    Err(_) => return false,
                }
            }
            true
        }
    }

    /// What every pod and the target source share with the run.
    #[derive(Default)]
    struct Shared {
        upstream: Mutex<Vec<String>>,
        /// Per request a pod received, its `authorization` value or `absent`.
        authorizations: Mutex<Vec<String>>,
        /// The owner secret, searched for in every byte a pod reads.
        secret: Vec<u8>,
        secret_upstream: AtomicBool,
        progress: Mutex<Arc<Progress>>,
        stopping: AtomicBool,
    }

    struct Pod {
        name: String,
        authority: String,
        script: Arc<Mutex<VecDeque<AnswerInput>>>,
        thread: Option<JoinHandle<()>>,
    }

    impl Pod {
        fn start(index: usize, script: Vec<AnswerInput>, shared: &Arc<Shared>) -> io::Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let authority = listener.local_addr()?.to_string();
            let name = format!("pod-{}", index + 1);
            let script = Arc::new(Mutex::new(VecDeque::from(script)));
            let (serving, recording, pod) = (Arc::clone(&script), Arc::clone(shared), name.clone());
            let thread = thread::spawn(move || {
                while let Ok((connection, _)) = listener.accept() {
                    if recording.stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    let answer = serving
                        .lock()
                        .ok()
                        .and_then(|mut script| script.pop_front());
                    answer_one(connection, answer, &pod, &recording);
                }
            });
            Ok(Self {
                name,
                authority,
                script,
                thread: Some(thread),
            })
        }
    }

    /// Reads one request, records it, and answers it as scripted.
    fn answer_one(
        mut connection: TcpStream,
        answer: Option<AnswerInput>,
        pod: &str,
        shared: &Shared,
    ) {
        let _timeouts = connection
            .set_read_timeout(Some(WAIT))
            .and_then(|()| connection.set_write_timeout(Some(WAIT)));
        let Some(answer) = answer else {
            return;
        };
        if let Some(seen) = read_request(&mut connection) {
            if !shared.secret.is_empty() && find(&seen.raw, &shared.secret).is_some() {
                shared.secret_upstream.store(true, Ordering::SeqCst);
            }
            if let Ok(mut upstream) = shared.upstream.lock() {
                upstream.push(format!("{pod} {}", seen.summary));
            }
            if let Ok(mut authorizations) = shared.authorizations.lock() {
                authorizations.push(seen.authorization.unwrap_or_else(|| "absent".to_owned()));
            }
        }
        let Some(status) = answer.status else {
            // `closed`: the request was read, and no byte of answer follows.
            let _closed = connection.shutdown(Shutdown::Both);
            return;
        };
        let progress = shared
            .progress
            .lock()
            .map(|progress| Arc::clone(&progress))
            .unwrap_or_default();
        let body: Vec<u8> = answer.chunks.concat().into_bytes();
        if let Ok(mut replied) = progress.replied.lock() {
            *replied = Some((status, body.clone()));
        }
        let chunked = answer.framing.as_deref() == Some("chunked");
        let framing = if chunked {
            "transfer-encoding: chunked\r\n".to_string()
        } else {
            format!("content-length: {}\r\n", body.len())
        };
        let content_type = answer
            .content_type
            .as_ref()
            .map_or(String::new(), |value| format!("content-type: {value}\r\n"));
        let head = format!(
            "HTTP/1.1 {status} Fixture\r\n{content_type}{framing}connection: close\r\n\r\n"
        );
        if connection.write_all(head.as_bytes()).is_err() {
            return;
        }
        let mut sent = 0;
        for (index, chunk) in answer.chunks.iter().enumerate() {
            if index > 0 && !progress.wait_for(sent) {
                progress.held.store(true, Ordering::SeqCst);
            }
            let mut framed = Vec::new();
            if chunked {
                framed.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
            }
            framed.extend_from_slice(chunk.as_bytes());
            if chunked {
                framed.extend_from_slice(b"\r\n");
            }
            if connection
                .write_all(&framed)
                .and_then(|()| connection.flush())
                .is_err()
            {
                return;
            }
            sent += chunk.len();
        }
        if chunked {
            let _end = connection.write_all(b"0\r\n\r\n");
        }
        let _closed = connection.shutdown(Shutdown::Both);
    }

    /// One request a pod read.
    struct Seen {
        /// `<method> <path> <body>`.
        summary: String,
        authorization: Option<String>,
        /// Every byte read for the request, head and body.
        raw: Vec<u8>,
    }

    /// One request, reading a `content-length` body.
    fn read_request(connection: &mut TcpStream) -> Option<Seen> {
        let mut buffer = Vec::new();
        let mut chunk = [0_u8; 16_384];
        let end = loop {
            if let Some(end) = find(&buffer, b"\r\n\r\n") {
                break end;
            }
            let read = connection.read(&mut chunk).ok()?;
            if read == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..read]);
        };
        let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
        let header = |wanted: &str| {
            head.split("\r\n")
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.trim().eq_ignore_ascii_case(wanted))
                .map(|(_, value)| value.trim().to_owned())
        };
        let length: usize = header("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut body = buffer[end + 4..].to_vec();
        while body.len() < length {
            let read = connection.read(&mut chunk).ok()?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..read]);
        }
        let raw = [&buffer[..end + 4], body.as_slice()].concat();
        let mut start = head.split("\r\n").next()?.split(' ');
        let (method, path) = (start.next()?, start.next()?);
        let text = if body.len() > RECORDED_BODY_BYTES {
            format!("{} bytes", body.len())
        } else {
            String::from_utf8_lossy(&body).into_owned()
        };
        Some(Seen {
            summary: format!("{method} {path} {text}"),
            authorization: header("authorization"),
            raw,
        })
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    /// Keeps every usage record the gateway hands out (rows O1, O2).
    #[derive(Default)]
    struct Records(Mutex<Vec<UsageRecord>>);

    impl UsageRecords for Records {
        fn record(&self, record: &UsageRecord) {
            if let Ok(mut records) = self.0.lock() {
                records.push(record.clone());
            }
        }
    }

    impl Records {
        /// The records made since the last call.
        fn take(&self) -> Vec<UsageRecord> {
            self.0
                .lock()
                .map(|mut records| std::mem::take(&mut *records))
                .unwrap_or_default()
        }
    }

    /// One record as the `records` fact writes it; `elapsed` is how long the client took from
    /// sending the request to reading its last byte.
    fn record_fact(record: &UsageRecord, elapsed: Duration) -> String {
        let duration = if u128::from(record.duration_ms) <= elapsed.as_millis() {
            "bounded".to_owned()
        } else {
            record.duration_ms.to_string()
        };
        let tokens = [
            record.input_tokens,
            record.output_tokens,
            record.cached_input_tokens,
            record.cache_creation_input_tokens,
            record.reasoning_output_tokens,
        ];
        let tokens = if tokens.iter().all(Option::is_none) && record.reported_model.is_none() {
            "absent"
        } else {
            "reported"
        };
        let disposition = match record.disposition {
            Disposition::Relayed => "Relayed",
            Disposition::Refused => "Refused",
            Disposition::UpstreamFailed => "UpstreamFailed",
        };
        format!(
            "model={} wire={} disposition={disposition} refusal={} status={} target_status={} response_bytes={} duration_ms={duration} tokens={tokens}",
            record.model.as_deref().unwrap_or("absent"),
            record.wire.label(),
            record.refusal.map_or("absent", |code| code.wire()),
            record.status,
            record
                .target_status
                .map_or_else(|| "absent".to_owned(), |status| status.to_string()),
            record.response_bytes,
        )
    }

    /// A pod as the source knows it: its name, its authority and its script.
    type PodSlot = (String, String, Arc<Mutex<VecDeque<AnswerInput>>>);

    /// The target source: reports a cold start for its first `cold` acquisitions, then hands
    /// out the current pod, and moves to the next once the gateway reports the current one
    /// failed.
    struct Source {
        pods: Vec<PodSlot>,
        /// Each model's bearer, set once the gateway is composed.
        bearers: Mutex<BTreeMap<String, Arc<TargetBearer>>>,
        current: Mutex<usize>,
        cold: AtomicUsize,
        acquired: AtomicUsize,
        released: Arc<AtomicUsize>,
        invalidated: Mutex<Vec<String>>,
    }

    struct Target {
        authority: String,
        script: Arc<Mutex<VecDeque<AnswerInput>>>,
        bearer: Option<Arc<TargetBearer>>,
        released: Arc<AtomicUsize>,
    }

    impl RelayTargets for Source {
        fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
            self.acquired.fetch_add(1, Ordering::SeqCst);
            if self
                .cold
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
            {
                return Err(TargetRefusal::ColdStart);
            }
            let current = *self
                .current
                .lock()
                .map_err(|_| TargetRefusal::Unavailable)?;
            let (_, authority, script) =
                self.pods.get(current).ok_or(TargetRefusal::Unavailable)?;
            let bearer = self
                .bearers
                .lock()
                .map_err(|_| TargetRefusal::Unavailable)?
                .get(alias)
                .cloned();
            Ok(Box::new(Target {
                authority: authority.clone(),
                script: Arc::clone(script),
                bearer,
                released: Arc::clone(&self.released),
            }))
        }

        fn invalidate(&self, _alias: &str, authority: &str) {
            let named = self.pods.iter().position(|(_, pod, _)| pod == authority);
            if let Ok(mut invalidated) = self.invalidated.lock() {
                invalidated.push(
                    named.map_or_else(|| authority.to_string(), |index| self.pods[index].0.clone()),
                );
            }
            if let Ok(mut current) = self.current.lock()
                && named == Some(*current)
            {
                *current += 1;
            }
        }
    }

    impl RelayTarget for Target {
        fn authority(&self) -> &str {
            &self.authority
        }

        fn bearer(&self) -> Option<&TargetBearer> {
            self.bearer.as_deref()
        }

        fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
            let refused = || io::Error::from(io::ErrorKind::ConnectionRefused);
            let mut script = self.script.lock().map_err(|_| refused())?;
            // A `refused` answer, or a connection past the script, never reaches the pod.
            match script.front() {
                None => return Err(refused()),
                Some(answer) if answer.transport.as_deref() == Some("refused") => {
                    script.pop_front();
                    return Err(refused());
                }
                Some(_) => {}
            }
            drop(script);
            let stream = TcpStream::connect(&self.authority)?;
            stream.set_read_timeout(Some(WAIT))?;
            stream.set_write_timeout(Some(WAIT))?;
            Ok(Box::new(stream))
        }
    }

    impl Drop for Target {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub(super) fn exercise(input: &Value) -> Result<Observed, TargetError> {
        let command: Command = serde_json::from_value(input.clone()).map_err(unavailable)?;
        Ok(Observed {
            facts: run(&command.program_json),
            view: "llm-gateway.gateway.LastRelay",
            event: "llm-gateway.gateway.Relayed",
            field: "valid_program",
        })
    }

    fn wire(name: &str) -> Result<Wire, String> {
        Wire::ALL
            .into_iter()
            .find(|wire| wire.label() == name)
            .ok_or_else(|| "fixture:unknown-wire".to_string())
    }

    fn relay_code(error: RelayError) -> &'static str {
        match error {
            RelayError::NoWire => "no-wire",
            RelayError::RepeatedWire => "repeated-wire",
            RelayError::DuplicateModel => "duplicate-model",
        }
    }

    fn compose(
        program: &Program,
        source: Arc<Source>,
        records: Arc<Records>,
    ) -> Result<GatewayHandle, String> {
        let token = OwnerToken::new(program.secret.as_bytes().to_vec())
            .map_err(|error| format!("token:{}", token_code(error)))?;
        let verifier = SharedSecretVerifier::new(token)
            .map_err(|error| format!("verifier:{}", verifier_code(error)))?;
        let relay_error = |error| format!("relay:{}", relay_code(error));
        let mut models = Vec::new();
        for model in &program.models {
            let wires = model
                .wires
                .iter()
                .map(|name| wire(name))
                .collect::<Result<Vec<_>, _>>()?;
            let tool_calling = match model.tool_calling.as_deref() {
                None | Some("absent") => ToolCalling::Absent,
                Some("parsed") => ToolCalling::Parsed,
                Some(_) => return Err("fixture:unknown-tool-calling".to_owned()),
            };
            models.push(
                RelayModel::new(label(&model.alias)?, label(&model.upstream_model)?, wires)
                    .map_err(relay_error)?
                    .with_tool_calling(tool_calling),
            );
        }
        let mut bearers = BTreeMap::new();
        for model in &program.models {
            if let Some(bearer) = &model.bearer {
                let bearer = TargetBearer::new(bearer.as_bytes().to_vec())
                    .map_err(|error| format!("bearer:{}", token_code(error)))?;
                bearers.insert(model.alias.clone(), Arc::new(bearer));
            }
        }
        *source
            .bearers
            .lock()
            .map_err(|_| "fixture:bearers".to_owned())? = bearers;
        let relay = Relay::new(models, source)
            .map_err(relay_error)?
            .with_records(records);
        let inventory =
            RouteInventory::new(Vec::new()).map_err(|_| "fixture:inventory".to_owned())?;
        let bind = "127.0.0.1:0"
            .parse()
            .map_err(|_| "fixture:bind-address".to_owned())?;
        let mut config = GatewayConfig::new(bind);
        config.read_timeout = WAIT;
        Gateway::bind_with_relay(config, Arc::new(verifier), inventory, relay)
            .map_err(|_| "bind:refused".to_owned())
    }

    /// The request a step sends, or `None` for a step the program cannot express.
    fn request_bytes(secret: &str, step: &Step) -> Option<Vec<u8>> {
        let Step::Request {
            path,
            body,
            method,
            credential,
            pad_to_bytes,
            declare_bytes,
        } = step
        else {
            return None;
        };
        let mut body = body.clone().into_bytes();
        if let Some(total) = *pad_to_bytes {
            let prefix = body.strip_suffix(b"}")?;
            let opening = [prefix, b",\"pad\":\""].concat();
            let fill = total.checked_sub(opening.len() + 2)?;
            body = [opening, vec![b'a'; fill], b"\"}".to_vec()].concat();
        }
        let auth = if credential.unwrap_or(true) {
            format!("authorization: Bearer {secret}\r\n")
        } else {
            String::new()
        };
        let length = declare_bytes.unwrap_or(u64::try_from(body.len()).ok()?);
        let mut raw = format!(
            "{} {path} HTTP/1.1\r\nhost: gateway\r\n{auth}content-type: application/json\r\ncontent-length: {length}\r\n\r\n",
            method.as_deref().unwrap_or("POST")
        )
        .into_bytes();
        if declare_bytes.is_none() {
            raw.extend_from_slice(&body);
        }
        Some(raw)
    }

    /// What the client received for one request.
    struct Received {
        status: Option<u16>,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl Received {
        fn header(&self, name: &str) -> &str {
            self.headers
                .iter()
                .find(|(key, _)| key == name)
                .map_or("absent", |(_, value)| value.as_str())
        }
    }

    /// Splits off the head, and decodes as much of the body as has arrived.
    fn decode(raw: &[u8]) -> Option<Received> {
        let end = find(raw, b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
        let mut lines = head.split("\r\n");
        let status = lines
            .next()
            .and_then(|line| line.split(' ').nth(1))
            .and_then(|status| status.parse().ok());
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let rest = &raw[end + 4..];
        let chunked = headers.iter().any(|(name, value)| {
            name == "transfer-encoding" && value.eq_ignore_ascii_case("chunked")
        });
        let body = if chunked {
            dechunk(rest)
        } else {
            rest.to_vec()
        };
        Some(Received {
            status,
            headers,
            body,
        })
    }

    fn dechunk(mut rest: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        while let Some(line_end) = find(rest, b"\r\n") {
            let size = String::from_utf8_lossy(&rest[..line_end]);
            let Ok(size) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16)
            else {
                break;
            };
            rest = &rest[line_end + 2..];
            if size == 0 {
                break;
            }
            let available = size.min(rest.len());
            body.extend_from_slice(&rest[..available]);
            if rest.len() < size + 2 {
                break;
            }
            rest = &rest[size + 2..];
        }
        body
    }

    /// Sends one request and reads its answer while it arrives, telling the pod how far the
    /// client has got.
    fn exchange(addr: SocketAddr, request: &[u8], progress: &Progress) -> Option<Received> {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream.set_read_timeout(Some(WAIT)).ok()?;
        stream.set_write_timeout(Some(WAIT)).ok()?;
        // A write the gateway cut short is not a lost answer: whatever arrived is read.
        let _written = stream.write_all(request).and_then(|()| stream.flush());
        let _half = stream.shutdown(Shutdown::Write);
        let mut raw = Vec::new();
        let mut chunk = [0_u8; 16_384];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(received) = decode(&raw) {
                        progress.set_received(received.body.len());
                    }
                }
            }
        }
        decode(&raw)
    }

    fn summary(received: &Received, progress: &Progress) -> String {
        let Some(status) = received.status else {
            return "no-response".to_owned();
        };
        let replied = progress
            .replied
            .lock()
            .ok()
            .and_then(|replied| replied.clone());
        if replied.as_ref() == Some(&(status, received.body.clone())) {
            return format!("{status} relayed");
        }
        let word = serde_json::from_slice::<Value>(&received.body)
            .ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "unparsed".to_owned());
        format!("{status} {word}")
    }

    fn valid(program: &Program) -> bool {
        program.steps.len() <= MAX_STEPS
            && program.cold_starts <= MAX_STEPS
            && program.pods.len() <= MAX_PODS
            && program
                .pods
                .iter()
                .all(|pod| pod.len() <= MAX_ANSWERS && pod.iter().all(AnswerInput::valid))
            && program.steps.iter().all(|step| match step {
                Step::MarkReady | Step::Scrape { .. } => true,
                Step::Request { pad_to_bytes, .. } => {
                    pad_to_bytes.is_none_or(|bytes| bytes <= MAX_PAD_BYTES)
                }
            })
    }

    fn run(program_json: &str) -> Value {
        let mut facts = json!({
            "valid_program": false, "error_code": null, "results": [], "bodies": [],
            "headers": [], "retry_after": [], "arrivals": [], "upstream": [], "acquired": 0, "invalidated": [],
            "released": 0, "authorizations": [], "secret_upstream": false, "records": [],
            "scrapes": []
        });
        if program_json.len() > MAX_PROGRAM_BYTES {
            return facts;
        }
        let Ok(program) = serde_json::from_str::<Program>(program_json) else {
            return facts;
        };
        if !valid(&program) {
            return facts;
        }
        facts["valid_program"] = json!(true);
        let shared = Arc::new(Shared {
            secret: program.secret.as_bytes().to_vec(),
            ..Shared::default()
        });
        let mut pods = Vec::new();
        for (index, script) in program.pods.iter().enumerate() {
            let Ok(pod) = Pod::start(index, script.clone(), &shared) else {
                facts["error_code"] = json!("fixture:pod");
                stop(&shared, pods);
                return facts;
            };
            pods.push(pod);
        }
        let source = Arc::new(Source {
            pods: pods
                .iter()
                .map(|pod| {
                    (
                        pod.name.clone(),
                        pod.authority.clone(),
                        Arc::clone(&pod.script),
                    )
                })
                .collect(),
            bearers: Mutex::new(BTreeMap::new()),
            current: Mutex::new(0),
            cold: AtomicUsize::new(program.cold_starts),
            acquired: AtomicUsize::new(0),
            released: Arc::new(AtomicUsize::new(0)),
            invalidated: Mutex::new(Vec::new()),
        });
        let records = Arc::new(Records::default());
        match compose(&program, Arc::clone(&source), Arc::clone(&records)) {
            Ok(handle) => {
                steps(&program, &handle, &shared, &records, &mut facts);
                handle.shutdown();
            }
            Err(code) => {
                facts["error_code"] = json!(code);
            }
        }
        stop(&shared, pods);
        facts["upstream"] = json!(
            shared
                .upstream
                .lock()
                .map(|seen| seen.clone())
                .unwrap_or_default()
        );
        facts["authorizations"] = json!(
            shared
                .authorizations
                .lock()
                .map(|seen| seen.clone())
                .unwrap_or_default()
        );
        facts["secret_upstream"] = json!(shared.secret_upstream.load(Ordering::SeqCst));
        facts["acquired"] = json!(source.acquired.load(Ordering::SeqCst));
        facts["invalidated"] = json!(
            source
                .invalidated
                .lock()
                .map(|seen| seen.clone())
                .unwrap_or_default()
        );
        facts["released"] = json!(source.released.load(Ordering::SeqCst));
        facts
    }

    /// A `GET /metrics`: its result entry, and its `scrapes` entries numbered `index`.
    fn scrape(
        addr: SocketAddr,
        secret: &str,
        credential: bool,
        index: usize,
        run_started: Instant,
        scrapes: &mut Vec<String>,
    ) -> String {
        let auth = if credential {
            format!("authorization: Bearer {secret}\r\n")
        } else {
            String::new()
        };
        let raw = format!("GET /metrics HTTP/1.1\r\nhost: gateway\r\n{auth}\r\n");
        let Some(received) = exchange(addr, raw.as_bytes(), &Progress::default()) else {
            return "no-response".to_owned();
        };
        let Some(status) = received.status else {
            return "no-response".to_owned();
        };
        scrapes.push(format!(
            "{index} {status} content-type={}",
            received.header("content-type")
        ));
        if status != 200 {
            return summary(&received, &Progress::default());
        }
        let text = String::from_utf8_lossy(&received.body).into_owned();
        scrapes.extend(
            text.lines()
                .filter(|line| !line.starts_with('#'))
                .map(|line| format!("{index} {}", waited(line, run_started.elapsed()))),
        );
        format!("{status} scraped")
    }

    /// A sample line as the `scrapes` fact writes it: the time requests waited for a starting
    /// target is measured, so its value is written `bounded` when it is at most the time the
    /// run has taken, and as served otherwise.
    fn waited(line: &str, run: Duration) -> String {
        const WAITED: &str = "llmgw_cold_start_wait_seconds_total ";
        let Some(value) = line.strip_prefix(WAITED) else {
            return line.to_owned();
        };
        match value.parse::<f64>() {
            Ok(seconds) if (0.0..=run.as_secs_f64()).contains(&seconds) => {
                format!("{WAITED}bounded")
            }
            _ => line.to_owned(),
        }
    }

    fn steps(
        program: &Program,
        handle: &GatewayHandle,
        shared: &Shared,
        records: &Records,
        facts: &mut Value,
    ) {
        let addr = handle.local_addr();
        let (mut results, mut bodies, mut headers, mut retry_after, mut arrivals) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut recorded, mut scrapes, mut scrape_number) = (Vec::new(), Vec::new(), 0);
        let run_started = Instant::now();
        for step in &program.steps {
            if matches!(step, Step::MarkReady) {
                handle.mark_ready();
                results.push("ok".to_owned());
                continue;
            }
            if let Step::Scrape { credential } = step {
                scrape_number += 1;
                let credential = credential.unwrap_or(true);
                results.push(scrape(
                    addr,
                    &program.secret,
                    credential,
                    scrape_number,
                    run_started,
                    &mut scrapes,
                ));
                continue;
            }
            let progress = Arc::new(Progress::default());
            if let Ok(mut current) = shared.progress.lock() {
                *current = Arc::clone(&progress);
            }
            let started = Instant::now();
            let received = request_bytes(&program.secret, step)
                .and_then(|raw| exchange(addr, &raw, &progress));
            // A record is made before the gateway closes the connection, so every record of
            // this request is kept by the time its answer has been read to the end.
            let elapsed = started.elapsed();
            recorded.extend(
                records
                    .take()
                    .iter()
                    .map(|record| record_fact(record, elapsed)),
            );
            let Some(received) = received else {
                results.push("no-response".to_owned());
                continue;
            };
            results.push(summary(&received, &progress));
            bodies.push(String::from_utf8_lossy(&received.body).into_owned());
            headers.push(format!(
                "content-type={} cache-control={} allow={}",
                received.header("content-type"),
                received.header("cache-control"),
                received.header("allow")
            ));
            retry_after.push(received.header("retry-after").to_owned());
            let replied = progress
                .replied
                .lock()
                .is_ok_and(|replied| replied.is_some());
            arrivals.push(match (replied, progress.held.load(Ordering::SeqCst)) {
                (false, _) => "none",
                (true, false) => "streamed",
                (true, true) => "held",
            });
        }
        facts["results"] = json!(results);
        facts["bodies"] = json!(bodies);
        facts["headers"] = json!(headers);
        facts["retry_after"] = json!(retry_after);
        facts["arrivals"] = json!(arrivals);
        facts["records"] = json!(recorded);
        facts["scrapes"] = json!(scrapes);
    }

    fn stop(shared: &Shared, pods: Vec<Pod>) {
        shared.stopping.store(true, Ordering::SeqCst);
        for mut pod in pods {
            // Unblocks the pod's `accept`, which then sees `stopping`.
            let _wake = TcpStream::connect(&pod.authority);
            if let Some(thread) = pod.thread.take() {
                let _joined = thread.join();
            }
        }
    }
}
