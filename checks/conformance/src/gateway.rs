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
    /// A deployment document's text: the inventory is the one the binary builds from it.
    #[serde(default)]
    document: Option<String>,
    /// Strings no byte the gateway writes may contain (`withheld_on_wire`).
    #[serde(default)]
    withheld: Vec<String>,
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
    let snapshot = match &program.document {
        // The binary's own loader and inventory; no file the document names is read.
        Some(document) => llm_gateway_cli::parse(document)
            .and_then(|deployment| llm_gateway_cli::inventory(&deployment))
            .map_err(|refusal| refusal.code().to_owned())?,
        None => inventory(program.inventory.as_ref().unwrap_or(&default_inventory()))?,
    };
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

/// The program `program_json` holds, or `None` when it is over its bounds, unparsed or names
/// both a document and an inventory.
fn valid_program(program_json: &str) -> Option<Program> {
    if program_json.len() > MAX_PROGRAM_BYTES {
        return None;
    }
    let program = serde_json::from_str::<Program>(program_json).ok()?;
    if program.steps.len() > MAX_STEPS
        || (program.document.is_some() && program.inventory.is_some())
        || program.withheld.iter().any(String::is_empty)
        || program
            .inventory
            .as_ref()
            .is_some_and(|inventory| inventory.generated_routes > MAX_GENERATED_ROUTES)
    {
        return None;
    }
    Some(program)
}

fn run(program_json: &str) -> Value {
    let mut facts = json!({
        "valid_program": false, "error_code": null, "results": [], "bodies": [],
        "headers": [], "answer_headers": [], "secret_on_wire": null, "withheld_on_wire": null,
        "ready": null, "shutdown": null
    });
    let Some(program) = valid_program(program_json) else {
        return facts;
    };
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
    let mut withheld_on_wire = false;
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
                withheld_on_wire |= program.withheld.iter().any(|withheld| {
                    answer
                        .raw
                        .windows(withheld.len())
                        .any(|window| window == withheld.as_bytes())
                });
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
    facts["withheld_on_wire"] = json!(withheld_on_wire);
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
        BasePath, CredentialHeader, CredentialHeaderError, Disposition, Gateway, GatewayConfig,
        GatewayHandle, IngressAuth, NoOwnerVerifier, OwnerToken, OwnerVerifier, ReasoningEffort,
        Relay, RelayError, RelayModel, RelayStream, RelayTarget, RelayTargets, RouteInventory,
        SharedSecretVerifier, StaticHeader, StaticHeaderError, TargetBearer, TargetRefusal,
        ToolCalling, UsageRecord, UsageRecords, Wire,
    };
    use llm_gateway::{ModelPhase, ModelStatus};
    use llm_gateway_cli::{
        EndpointCredential, HostedEndpoint, HostedTargets, LlmTranslator, PodConnector,
        ProxyConnector, Routes,
    };
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer,
        KeyPair, KeyUsagePurpose,
    };
    use rustls::{
        ServerConfig, ServerConnection, StreamOwned,
        pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
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
    /// How long a pod whose client closed early waits to see the gateway close the upstream
    /// connection before it releases its next chunk.
    const CLOSE_WAIT: Duration = Duration::from_millis(500);
    /// The translator's fixed `created`, so a translated answer's bytes are exact.
    const CREATED: u64 = 1_800_000_000;

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
        /// `bearer`, the default, or `none`: the Gateway's `ingress_auth`.
        #[serde(default)]
        ingress_auth: Option<String>,
        /// `off`, the default, or `on`: the relay is composed with the binary's translator.
        #[serde(default)]
        translation: Option<String>,
        /// The text of an `llm.routes/1` document whose routes are served beside `models`; each
        /// `//pod-<n>` in it is replaced by `//` and that pod's authority before it is parsed.
        #[serde(default)]
        routes_document: Option<String>,
        /// Each credentialed route endpoint's credential material, by endpoint name: no file
        /// the routes document names is read.
        #[serde(default)]
        route_credentials: BTreeMap<String, String>,
        /// The source's answers to the lifecycle routes for the models its pods serve; without
        /// it the source starts no pod for any model.
        #[serde(default)]
        lifecycle: Option<LifecycleInput>,
    }

    /// The source's scripted lifecycle answers, one per ask, in order.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LifecycleInput {
        #[serde(default)]
        wake: Vec<LifecycleAnswer>,
        #[serde(default)]
        status: Vec<LifecycleAnswer>,
    }

    /// A `ModelStatus`, or for a wake a refusal named by its code.
    #[derive(Deserialize, Clone)]
    #[serde(deny_unknown_fields)]
    struct LifecycleAnswer {
        #[serde(default)]
        phase: Option<String>,
        #[serde(default)]
        sub_phase: Option<String>,
        #[serde(default)]
        profile: Option<String>,
        #[serde(default)]
        refusal: Option<String>,
    }

    impl LifecycleAnswer {
        fn status(&self) -> Option<ModelStatus> {
            let code = self.phase.as_deref()?;
            let phase = ModelPhase::ALL
                .into_iter()
                .find(|phase| phase.code() == code)?;
            Some(ModelStatus {
                phase,
                sub_phase: self.sub_phase.clone(),
                profile: self.profile.clone(),
            })
        }

        fn refused(&self) -> Option<TargetRefusal> {
            let code = self.refusal.as_deref()?;
            TargetRefusal::ALL
                .iter()
                .copied()
                .find(|refusal| refusal.refusal().wire() == code)
        }

        /// A status, or (`may_refuse`) a refusal, and never both.
        fn valid(&self, may_refuse: bool) -> bool {
            match (&self.refusal, self.status()) {
                (None, Some(_)) => true,
                (Some(_), None) => {
                    may_refuse
                        && self.refused().is_some()
                        && self.sub_phase.is_none()
                        && self.profile.is_none()
                }
                _ => false,
            }
        }
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
        /// What `GET /v1/models` lists as `max_model_len` (row R5).
        #[serde(default)]
        max_model_len: Option<u64>,
        /// The `max_model_len` every target handed out for this model states, which the output
        /// clamp takes before the model's own window.
        #[serde(default)]
        target_max_model_len: Option<u64>,
        /// What `GET /` tells each client as its context window (row R1).
        #[serde(default)]
        context_window: Option<u64>,
        /// `low`, `medium` or `xhigh`: the effort a messages request naming none is given.
        #[serde(default)]
        reasoning_effort: Option<String>,
        /// Served by this hosted endpoint, through the binary's hosted target source, instead
        /// of the source's current pod.
        #[serde(default)]
        endpoint: Option<EndpointInput>,
    }

    /// A hosted endpoint at a fixture pod's address (llm-gateway.upstream.HostedEndpoint).
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EndpointInput {
        /// The pod at the endpoint's authority: `pod-1`, `pod-2`, …
        pod: String,
        /// `http` or `https`; an `https` endpoint's pod serves TLS.
        scheme: String,
        /// Appended to `<scheme>://<authority>` to make the endpoint.s `base_url`.
        #[serde(default)]
        base_path: Option<String>,
        /// `trusted`, the default, or `untrusted`: whose root issued the pod's certificate.
        #[serde(default)]
        certificate: Option<String>,
        auth_kind: String,
        /// The credential's material.
        #[serde(default)]
        credential: Option<String>,
        #[serde(default)]
        credential_header: Option<String>,
        /// Static header names to values.
        #[serde(default)]
        headers: BTreeMap<String, String>,
    }

    impl EndpointInput {
        /// The pod's index, when `pod` names one of `pods` of them.
        fn pod_index(&self, pods: usize) -> Option<usize> {
            let number: usize = self.pod.strip_prefix("pod-")?.parse().ok()?;
            (1..=pods).contains(&number).then(|| number - 1)
        }

        /// Whose root issued the pod's certificate, for an `https` endpoint.
        fn tls(&self) -> Option<bool> {
            (self.scheme == "https").then(|| self.certificate.as_deref() != Some("untrusted"))
        }

        fn valid(&self, pods: usize) -> bool {
            let credential = match self.auth_kind.as_str() {
                "anonymous" => self.credential.is_none() && self.credential_header.is_none(),
                "bearer" => self.credential.is_some() && self.credential_header.is_none(),
                "api-key" => self.credential.is_some() && self.credential_header.is_some(),
                _ => false,
            };
            credential
                && self.pod_index(pods).is_some()
                && matches!(self.scheme.as_str(), "http" | "https")
                && match self.certificate.as_deref() {
                    None => true,
                    Some("trusted" | "untrusted") => self.scheme == "https",
                    Some(_) => false,
                }
        }
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
        /// `received`, the default, or `progress`: when each chunk after the first is released.
        #[serde(default)]
        release: Option<String>,
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
            /// Sent as the request's `user-agent` (row R1); none is sent without it.
            #[serde(default)]
            user_agent: Option<String>,
            /// `<name>: <value>` lines sent after `host`, as a client such as Claude Code adds
            /// `anthropic-version`.
            #[serde(default)]
            headers: Vec<String>,
            /// The client closes the connection once it has received this many body bytes.
            #[serde(default)]
            close_after_bytes: Option<usize>,
            /// Whether the client shuts its sending side once the request is written; default true.
            #[serde(default)]
            half_close: Option<bool>,
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
                        && matches!(
                            self.release.as_deref(),
                            None | Some("received" | "progress")
                        )
                }
                _ => false,
            }
        }

        /// Whether each chunk waits for the client's progress rather than for its bytes.
        fn by_progress(&self) -> bool {
            self.release.as_deref() == Some("progress")
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
        /// The client closed its connection early (`close_after_bytes`).
        closed: AtomicBool,
    }

    impl Progress {
        fn set_received(&self, count: usize) {
            if let Ok(mut received) = self.received.lock() {
                *received = count;
            }
            self.arrived.notify_all();
        }

        fn received(&self) -> usize {
            self.received.lock().map_or(0, |received| *received)
        }

        fn close(&self) {
            // Set under the lock a waiting pod checks it under, so the wake is not lost.
            let _guard = self.received.lock();
            self.closed.store(true, Ordering::SeqCst);
            self.arrived.notify_all();
        }

        fn is_closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }

        /// Waits until the client has received more than `baseline` body bytes, or closed;
        /// false if neither happened in time.
        fn wait_beyond(&self, baseline: usize) -> bool {
            let deadline = Instant::now() + CHUNK_HOLD;
            let Ok(mut received) = self.received.lock() else {
                return false;
            };
            while *received <= baseline && !self.is_closed() {
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
        /// Per request a pod received, its header lines but `host`, joined by ` | `.
        received_heads: Mutex<Vec<String>>,
        /// Per connection a pod accepted, `<pod> <transport> <request|nothing>`.
        connections: Mutex<Vec<String>>,
        /// Per reply a pod began, `<pod> complete` or `<pod> cut`.
        ends: Mutex<Vec<String>>,
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
        /// A loopback pod; with `tls`, every connection is served over TLS with that
        /// configuration.
        fn start(
            index: usize,
            script: Vec<AnswerInput>,
            shared: &Arc<Shared>,
            tls: Option<Arc<ServerConfig>>,
        ) -> io::Result<Self> {
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
                    answer_one(connection, answer, &pod, &recording, tls.as_ref());
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

    /// One accepted connection, plain or TLS.
    enum Connection {
        Plain(TcpStream),
        Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
    }

    impl Read for Connection {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            match self {
                Self::Plain(stream) => stream.read(buffer),
                Self::Tls(stream) => stream.read(buffer),
            }
        }
    }

    impl Write for Connection {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            match self {
                Self::Plain(stream) => stream.write(buffer),
                Self::Tls(stream) => stream.write(buffer),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self {
                Self::Plain(stream) => stream.flush(),
                Self::Tls(stream) => stream.flush(),
            }
        }
    }

    impl Connection {
        fn transport(&self) -> &'static str {
            match self {
                Self::Plain(_) => "plain",
                Self::Tls(_) => "tls",
            }
        }

        /// Whether the gateway closed this connection, waiting up to [`CLOSE_WAIT`] to see it.
        fn peer_closed(&mut self) -> bool {
            let socket = match self {
                Self::Plain(stream) => &*stream,
                Self::Tls(stream) => &stream.sock,
            };
            if socket.set_read_timeout(Some(CLOSE_WAIT)).is_err() {
                return true;
            }
            let mut byte = [0_u8; 1];
            let closed = match self.read(&mut byte) {
                Ok(0) => true,
                Ok(_) => false,
                Err(error) => !matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ),
            };
            let socket = match self {
                Self::Plain(stream) => &*stream,
                Self::Tls(stream) => &stream.sock,
            };
            let _restored = socket.set_read_timeout(Some(WAIT));
            closed
        }

        /// Ends the connection: a TLS close first, then both directions of the socket.
        fn close(self) {
            let socket = match self {
                Self::Plain(stream) => stream,
                Self::Tls(mut stream) => {
                    stream.conn.send_close_notify();
                    let _flushed = stream.flush();
                    stream.sock
                }
            };
            let _closed = socket.shutdown(Shutdown::Both);
        }
    }

    fn record(list: &Mutex<Vec<String>>, entry: String) {
        if let Ok(mut list) = list.lock() {
            list.push(entry);
        }
    }

    /// The TLS handshake on `socket`; on failure the connection is recorded `tls-refused` with
    /// whatever request bytes were decrypted (none are expected) and `None` returned.
    fn handshake(
        socket: TcpStream,
        config: &Arc<ServerConfig>,
        pod: &str,
        shared: &Shared,
    ) -> Option<Connection> {
        let refused = |read: bool| {
            record(
                &shared.connections,
                format!(
                    "{pod} tls-refused {}",
                    if read { "request" } else { "nothing" }
                ),
            );
        };
        let Ok(server) = ServerConnection::new(Arc::clone(config)) else {
            refused(false);
            return None;
        };
        let mut stream = StreamOwned::new(server, socket);
        while stream.conn.is_handshaking() {
            if stream.conn.complete_io(&mut stream.sock).is_err() {
                let mut decrypted = Vec::new();
                let _read = stream.conn.reader().read_to_end(&mut decrypted);
                refused(!decrypted.is_empty());
                let _closed = stream.sock.shutdown(Shutdown::Both);
                return None;
            }
        }
        Some(Connection::Tls(Box::new(stream)))
    }

    /// Reads one request, records it, and answers it as scripted.
    fn answer_one(
        socket: TcpStream,
        answer: Option<AnswerInput>,
        pod: &str,
        shared: &Shared,
        tls: Option<&Arc<ServerConfig>>,
    ) {
        let _timeouts = socket
            .set_read_timeout(Some(WAIT))
            .and_then(|()| socket.set_write_timeout(Some(WAIT)));
        // Past the script, or a refused connection: closed unread and not recorded. A source's
        // target refuses such a connection before it is opened; a hosted endpoint opens it.
        let Some(answer) = answer.filter(|answer| answer.transport.as_deref() != Some("refused"))
        else {
            let _closed = socket.shutdown(Shutdown::Both);
            return;
        };
        let mut connection = match tls {
            Some(config) => {
                let Some(connection) = handshake(socket, config, pod, shared) else {
                    return;
                };
                connection
            }
            None => Connection::Plain(socket),
        };
        let (seen, read) = read_request(&mut connection);
        record(
            &shared.connections,
            format!(
                "{pod} {} {}",
                connection.transport(),
                if read { "request" } else { "nothing" }
            ),
        );
        if let Some(seen) = seen {
            if !shared.secret.is_empty() && find(&seen.raw, &shared.secret).is_some() {
                shared.secret_upstream.store(true, Ordering::SeqCst);
            }
            record(&shared.upstream, format!("{pod} {}", seen.summary));
            record(
                &shared.authorizations,
                seen.authorization.unwrap_or_else(|| "absent".to_owned()),
            );
            record(&shared.received_heads, seen.head_lines);
        }
        let Some(status) = answer.status else {
            // `closed`: the request was read, and no byte of answer follows.
            connection.close();
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
        let cut = !write_chunks(&mut connection, &answer, &progress, chunked);
        record(
            &shared.ends,
            format!("{pod} {}", if cut { "cut" } else { "complete" }),
        );
        connection.close();
    }

    /// Writes the answer's chunks, each released as its `release` says; `false` when the
    /// connection closed before the last one was written.
    fn write_chunks(
        connection: &mut Connection,
        answer: &AnswerInput,
        progress: &Progress,
        chunked: bool,
    ) -> bool {
        let mut sent = 0;
        let mut baseline = 0;
        let mut cut = false;
        for (index, chunk) in answer.chunks.iter().enumerate() {
            if index > 0 && answer.by_progress() {
                if !progress.wait_beyond(baseline) {
                    progress.held.store(true, Ordering::SeqCst);
                }
                // A client gone early releases the next chunk at once, unless the gateway has
                // already closed the upstream connection: the turn was cancelled.
                if progress.is_closed() && connection.peer_closed() {
                    cut = true;
                    break;
                }
            } else if index > 0 && !progress.wait_for(sent) {
                progress.held.store(true, Ordering::SeqCst);
            }
            baseline = progress.received();
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
                cut = true;
                break;
            }
            sent += chunk.len();
        }
        if !cut && chunked && connection.write_all(b"0\r\n\r\n").is_err() {
            cut = true;
        }
        !cut
    }

    /// One request a pod read.
    struct Seen {
        /// `<method> <path> <body>`.
        summary: String,
        authorization: Option<String>,
        /// Every header line but `host`, as received and in order, joined by ` | `.
        head_lines: String,
        /// Every byte read for the request, head and body.
        raw: Vec<u8>,
    }

    /// One request, reading a `content-length` body, and whether any byte was read at all.
    fn read_request(connection: &mut impl Read) -> (Option<Seen>, bool) {
        let mut buffer = Vec::new();
        let seen = read_into(connection, &mut buffer);
        (seen, !buffer.is_empty())
    }

    fn read_into(connection: &mut impl Read, buffer: &mut Vec<u8>) -> Option<Seen> {
        let mut chunk = [0_u8; 16_384];
        let end = loop {
            if let Some(end) = find(buffer, b"\r\n\r\n") {
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
        let head_lines = head
            .split("\r\n")
            .skip(1)
            .filter(|line| {
                line.split_once(':')
                    .is_none_or(|(name, _)| !name.trim().eq_ignore_ascii_case("host"))
            })
            .collect::<Vec<_>>()
            .join(" | ");
        Some(Seen {
            summary: format!("{method} {path} {text}"),
            authorization: header("authorization"),
            head_lines,
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
        /// The `max_model_len` each model's targets state.
        windows: BTreeMap<String, u64>,
        current: Mutex<usize>,
        cold: AtomicUsize,
        /// The scripted wake and status answers; `None` starts no pod for any model.
        lifecycle: Option<Mutex<(VecDeque<LifecycleAnswer>, VecDeque<LifecycleAnswer>)>>,
    }

    struct Target {
        authority: String,
        script: Arc<Mutex<VecDeque<AnswerInput>>>,
        bearer: Option<Arc<TargetBearer>>,
        max_model_len: Option<u64>,
    }

    impl RelayTargets for Source {
        fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
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
                max_model_len: self.windows.get(alias).copied(),
            }))
        }

        fn invalidate(&self, _alias: &str, authority: &str) {
            let named = self.pods.iter().position(|(_, pod, _)| pod == authority);
            if let Ok(mut current) = self.current.lock()
                && named == Some(*current)
            {
                *current += 1;
            }
        }

        /// The next scripted wake answer; past the script, `target-unavailable`.
        fn wake(&self, _alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
            let next = self.lifecycle.as_ref()?.lock().ok()?.0.pop_front();
            Some(match next {
                Some(answer) => answer
                    .status()
                    .ok_or_else(|| answer.refused().unwrap_or(TargetRefusal::Unavailable)),
                None => Err(TargetRefusal::Unavailable),
            })
        }

        /// The next scripted status; past the script, `stopped`.
        fn status(&self, _alias: &str) -> Option<ModelStatus> {
            let next = self.lifecycle.as_ref()?.lock().ok()?.1.pop_front();
            Some(
                next.and_then(|answer| answer.status())
                    .unwrap_or(ModelStatus::new(ModelPhase::Stopped)),
            )
        }
    }

    impl RelayTarget for Target {
        fn authority(&self) -> &str {
            &self.authority
        }

        fn bearer(&self) -> Option<&TargetBearer> {
            self.bearer.as_deref()
        }

        fn max_model_len(&self) -> Option<u64> {
            self.max_model_len
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

    /// What the gateway asked of its target source, counted where the gateway asks it: above
    /// the binary's hosted target source, which serves the hosted models and hands every other
    /// alias to [`Source`].
    struct Counting {
        inner: Arc<dyn RelayTargets>,
        /// Each pod's name and authority, to name an invalidated one.
        names: Vec<(String, String)>,
        acquired: AtomicUsize,
        released: Arc<AtomicUsize>,
        invalidated: Mutex<Vec<String>>,
        /// Every lifecycle ask, as `wake <alias>` or `status <alias>`.
        lifecycle_asks: Mutex<Vec<String>>,
    }

    /// A target the gateway holds, counted released when it is dropped.
    struct Counted {
        inner: Box<dyn RelayTarget>,
        released: Arc<AtomicUsize>,
    }

    impl RelayTargets for Counting {
        fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
            self.acquired.fetch_add(1, Ordering::SeqCst);
            let inner = self.inner.acquire(alias)?;
            Ok(Box::new(Counted {
                inner,
                released: Arc::clone(&self.released),
            }))
        }

        fn invalidate(&self, alias: &str, authority: &str) {
            let named = self
                .names
                .iter()
                .find(|(_, pod)| pod == authority)
                .map_or_else(|| authority.to_owned(), |(name, _)| name.clone());
            record(&self.invalidated, named);
            self.inner.invalidate(alias, authority);
        }

        fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
            record(&self.lifecycle_asks, format!("wake {alias}"));
            self.inner.wake(alias)
        }

        fn status(&self, alias: &str) -> Option<ModelStatus> {
            record(&self.lifecycle_asks, format!("status {alias}"));
            self.inner.status(alias)
        }
    }

    impl RelayTarget for Counted {
        fn authority(&self) -> &str {
            self.inner.authority()
        }

        fn bearer(&self) -> Option<&TargetBearer> {
            self.inner.bearer()
        }

        fn credential_header(&self) -> Option<&CredentialHeader> {
            self.inner.credential_header()
        }

        fn base_path(&self) -> Option<&BasePath> {
            self.inner.base_path()
        }

        fn headers(&self) -> &[StaticHeader] {
            self.inner.headers()
        }

        fn held(&self) -> bool {
            self.inner.held()
        }

        fn max_model_len(&self) -> Option<u64> {
            self.inner.max_model_len()
        }

        fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
            self.inner.connect()
        }
    }

    impl Drop for Counted {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// The run's TLS material: one test root, the only root the client trusts, and server
    /// configurations presenting a certificate for `127.0.0.1` from it and from a stranger root.
    struct Certificates {
        root: Vec<u8>,
        trusted: Arc<ServerConfig>,
        untrusted: Arc<ServerConfig>,
    }

    fn authority_root(name: &str) -> Option<(Vec<u8>, Issuer<'static, KeyPair>)> {
        let mut params = CertificateParams::new(Vec::<String>::new()).ok()?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name.push(DnType::CommonName, name);
        params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let key = KeyPair::generate().ok()?;
        let certificate = params.self_signed(&key).ok()?;
        Some((certificate.der().to_vec(), Issuer::new(params, key)))
    }

    fn server_for_loopback(issuer: &Issuer<'static, KeyPair>) -> Option<Arc<ServerConfig>> {
        let mut params = CertificateParams::new(vec!["127.0.0.1".to_owned()]).ok()?;
        params
            .distinguished_name
            .push(DnType::CommonName, "127.0.0.1");
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().ok()?;
        let certificate = params.signed_by(&key, issuer).ok()?;
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .ok()?
                .with_no_client_auth()
                .with_single_cert(
                    vec![certificate.der().clone()],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
                )
                .ok()?;
        Some(Arc::new(config))
    }

    fn certificates() -> Option<Certificates> {
        let (root, issuer) = authority_root("conformance trusted root")?;
        let (_, stranger) = authority_root("conformance untrusted root")?;
        Some(Certificates {
            root,
            trusted: server_for_loopback(&issuer)?,
            untrusted: server_for_loopback(&stranger)?,
        })
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

    fn static_header_code(error: StaticHeaderError) -> &'static str {
        match error {
            StaticHeaderError::NameLength => "name-length",
            StaticHeaderError::NameNotAToken => "name-not-a-token",
            StaticHeaderError::Reserved => "reserved",
            StaticHeaderError::ValueLength => "value-length",
            StaticHeaderError::ValueNotPrintable => "value-not-printable",
        }
    }

    fn credential_header_code(error: CredentialHeaderError) -> &'static str {
        match error {
            CredentialHeaderError::Length => "length",
            CredentialHeaderError::NotAToken => "not-a-token",
            CredentialHeaderError::Reserved => "reserved",
        }
    }

    /// Each hosted model's endpoint, at its pod's address, as the binary builds one.
    fn endpoints(
        program: &Program,
        authorities: &[String],
    ) -> Result<BTreeMap<String, HostedEndpoint>, String> {
        let bearer = |material: Option<&String>| {
            TargetBearer::new(material.map_or_else(Vec::new, |text| text.as_bytes().to_vec()))
                .map_err(|error| format!("bearer:{}", token_code(error)))
        };
        let mut endpoints = BTreeMap::new();
        for model in &program.models {
            let Some(endpoint) = &model.endpoint else {
                continue;
            };
            let credential = match endpoint.auth_kind.as_str() {
                "anonymous" => EndpointCredential::Anonymous,
                "bearer" => EndpointCredential::Bearer(bearer(endpoint.credential.as_ref())?),
                _ => EndpointCredential::ApiKey {
                    header: CredentialHeader::new(
                        endpoint.credential_header.as_deref().unwrap_or_default(),
                    )
                    .map_err(|error| {
                        format!("credential-header:{}", credential_header_code(error))
                    })?,
                    key: bearer(endpoint.credential.as_ref())?,
                },
            };
            let authority = endpoint
                .pod_index(authorities.len())
                .and_then(|index| authorities.get(index))
                .ok_or_else(|| "fixture:endpoint-pod".to_owned())?;
            let base_url = format!(
                "{}://{authority}{}",
                endpoint.scheme,
                endpoint.base_path.as_deref().unwrap_or_default()
            );
            let mut headers = endpoint
                .headers
                .iter()
                .map(|(name, value)| {
                    StaticHeader::new(name, value)
                        .map_err(|error| format!("static-header:{}", static_header_code(error)))
                })
                .collect::<Result<Vec<_>, _>>()?;
            headers.sort_by(|left, right| left.name().cmp(right.name()));
            let hosted = HostedEndpoint::new(&base_url, credential)
                .map_err(|_| "endpoint:base-url".to_owned())?
                .with_headers(headers)
                .map_err(|_| "endpoint:headers".to_owned())?;
            endpoints.insert(model.alias.clone(), hosted);
        }
        Ok(endpoints)
    }

    /// The routes document, empty without `routes_document`: each `//pod-<n>` replaced by `//`
    /// and that pod's authority, then parsed as the binary parses it. A route name equal to a
    /// model's alias is refused as the binary refuses one equal to a `[hosted.*]` alias.
    fn routes(program: &Program, authorities: &[String]) -> Result<Routes, String> {
        let Some(text) = &program.routes_document else {
            return Ok(Routes::default());
        };
        let mut text = text.clone();
        // The highest pod first, so `//pod-1` never rewrites the start of `//pod-10`.
        for (index, authority) in authorities.iter().enumerate().rev() {
            text = text.replace(&format!("//pod-{}", index + 1), &format!("//{authority}"));
        }
        let routes = Routes::parse(&text).map_err(|refusal| refusal.code().to_owned())?;
        if routes.routes().any(|route| {
            program
                .models
                .iter()
                .any(|model| model.alias == route.name())
        }) {
            return Err("config:value".to_owned());
        }
        Ok(routes)
    }

    /// The binary's translator, on a fixed clock so a translated answer's bytes are exact, each
    /// route translated under its capabilities.
    fn translator(routes: &Routes) -> LlmTranslator {
        routes
            .translator(LlmTranslator::new())
            .with_created(CREATED)
    }

    /// Serves the routes document's routes beside the program's models, as the binary composes
    /// them (story:routes-document): each route's hosted endpoint, sent the credential
    /// `route_credentials` gives its endpoint, and its relayed model.
    fn serve_routes(
        program: &Program,
        authorities: &[String],
        endpoints: &mut BTreeMap<String, HostedEndpoint>,
        models: &mut Vec<RelayModel>,
    ) -> Result<Routes, String> {
        let routes = routes(program, authorities)?;
        let code = |refusal: llm_gateway_cli::Refusal| refusal.code().to_owned();
        let mut credentials = BTreeMap::new();
        for (endpoint, material) in &program.route_credentials {
            let bearer = TargetBearer::new(material.as_bytes().to_vec())
                .map_err(|error| format!("bearer:{}", token_code(error)))?;
            credentials.insert(endpoint.clone(), bearer);
        }
        endpoints.extend(routes.hosted_endpoints(&credentials).map_err(code)?);
        models.extend(routes.relay_models().map_err(code)?);
        Ok(routes)
    }

    fn compose(
        program: &Program,
        source: Arc<Source>,
        counting: &mut Option<Arc<Counting>>,
        records: Arc<Records>,
        tls_root: Option<&[u8]>,
    ) -> Result<GatewayHandle, String> {
        let ingress_auth = match program.ingress_auth.as_deref() {
            None | Some("bearer") => IngressAuth::Bearer,
            Some("none") => IngressAuth::None,
            Some(_) => return Err("fixture:unknown-ingress-auth".to_owned()),
        };
        // With ingress auth off and no secret the gateway has no owner at all.
        let verifier: Arc<dyn OwnerVerifier> =
            if ingress_auth == IngressAuth::None && program.secret.is_empty() {
                Arc::new(NoOwnerVerifier)
            } else {
                let token = OwnerToken::new(program.secret.as_bytes().to_vec())
                    .map_err(|error| format!("token:{}", token_code(error)))?;
                Arc::new(
                    SharedSecretVerifier::new(token)
                        .map_err(|error| format!("verifier:{}", verifier_code(error)))?,
                )
            };
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
            let mut relayed =
                RelayModel::new(label(&model.alias)?, label(&model.upstream_model)?, wires)
                    .map_err(relay_error)?
                    .with_tool_calling(tool_calling);
            if let Some(length) = model.max_model_len {
                relayed = relayed.with_max_model_len(length);
            }
            if let Some(window) = model.context_window {
                relayed = relayed.with_context_window(window);
            }
            if let Some(effort) = model.reasoning_effort.as_deref() {
                let effort = ReasoningEffort::from_name(effort)
                    .ok_or_else(|| "fixture:unknown-reasoning-effort".to_owned())?;
                relayed = relayed.with_reasoning_effort(effort);
            }
            models.push(relayed);
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
        let names: Vec<(String, String)> = source
            .pods
            .iter()
            .map(|(name, authority, _)| (name.clone(), authority.clone()))
            .collect();
        let authorities: Vec<String> = names.iter().map(|(_, pod)| pod.clone()).collect();
        let mut endpoints = endpoints(program, &authorities)?;
        let routes = serve_routes(program, &authorities, &mut endpoints, &mut models)?;
        // The binary's TLS client over the run's test root: the seam the shipped binary cannot
        // reach. A run with no https endpoint trusts nothing, and connects over TLS to nothing.
        let tls: Arc<dyn PodConnector> = match tls_root {
            Some(root) => Arc::new(
                ProxyConnector::with_test_seam(&[root.to_vec()], None)
                    .map_err(|_| "fixture:tls-root".to_owned())?,
            ),
            None => Arc::new(ProxyConnector::shipped()),
        };
        let targets = Arc::new(Counting {
            inner: Arc::new(HostedTargets::new(endpoints, tls, source)),
            names,
            acquired: AtomicUsize::new(0),
            released: Arc::new(AtomicUsize::new(0)),
            invalidated: Mutex::new(Vec::new()),
            lifecycle_asks: Mutex::new(Vec::new()),
        });
        *counting = Some(Arc::clone(&targets));
        let relay = Relay::new(models, targets)
            .map_err(relay_error)?
            .with_records(records);
        // The binary's translator, on a fixed clock so a translated answer's bytes are exact.
        let relay = match program.translation.as_deref() {
            None | Some("off") => relay,
            Some("on") => relay.with_translator(Arc::new(translator(&routes))),
            Some(_) => return Err("fixture:unknown-translation".to_owned()),
        };
        let inventory =
            RouteInventory::new(Vec::new()).map_err(|_| "fixture:inventory".to_owned())?;
        let bind = "127.0.0.1:0"
            .parse()
            .map_err(|_| "fixture:bind-address".to_owned())?;
        let mut config = GatewayConfig::new(bind);
        config.read_timeout = WAIT;
        config.ingress_auth = ingress_auth;
        Gateway::bind_with_relay(config, verifier, inventory, relay)
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
            user_agent,
            headers,
            ..
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
        let agent = user_agent
            .as_ref()
            .map_or_else(String::new, |agent| format!("user-agent: {agent}\r\n"));
        let extra = headers.iter().fold(String::new(), |mut extra, line| {
            extra.push_str(line);
            extra.push_str("\r\n");
            extra
        });
        let length = declare_bytes.unwrap_or(u64::try_from(body.len()).ok()?);
        let mut raw = format!(
            "{} {path} HTTP/1.1\r\nhost: gateway\r\n{extra}{agent}{auth}content-type: application/json\r\ncontent-length: {length}\r\n\r\n",
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
    /// With `close_after`, the client closes the connection once it has received that many body
    /// bytes, and tells the pod through `progress`.
    fn exchange(
        addr: SocketAddr,
        request: &[u8],
        progress: &Progress,
        close_after: Option<usize>,
        half_close: bool,
    ) -> Option<Received> {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream.set_read_timeout(Some(WAIT)).ok()?;
        stream.set_write_timeout(Some(WAIT)).ok()?;
        // A write the gateway cut short is not a lost answer: whatever arrived is read.
        let _written = stream.write_all(request).and_then(|()| stream.flush());
        if half_close {
            let _half = stream.shutdown(Shutdown::Write);
        }
        let mut raw = Vec::new();
        let mut chunk = [0_u8; 16_384];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    raw.extend_from_slice(&chunk[..read]);
                    if let Some(received) = decode(&raw) {
                        progress.set_received(received.body.len());
                        if close_after.is_some_and(|bytes| received.body.len() >= bytes) {
                            drop(stream);
                            progress.close();
                            return Some(received);
                        }
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
        if progress.is_closed() {
            return format!("{status} closed");
        }
        if replied.as_ref() == Some(&(status, received.body.clone())) {
            return format!("{status} relayed");
        }
        let refusal = serde_json::from_slice::<Value>(&received.body)
            .ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_owned));
        // An answer a pod replied to that reached the client in other bytes: a translation.
        // One the gateway wrote itself that is no refusal: a public route (R1, R5).
        let word = match refusal {
            Some(code) => code,
            None if (200..300).contains(&status) && replied.is_some() => "translated".to_owned(),
            None if (200..300).contains(&status) => "answered".to_owned(),
            None => "unparsed".to_owned(),
        };
        format!("{status} {word}")
    }

    fn valid(program: &Program) -> bool {
        let pods = program.pods.len();
        // A pod serves one transport: every endpoint naming it agrees on its TLS.
        let mut tls: BTreeMap<usize, Option<bool>> = BTreeMap::new();
        let endpoints_agree = program.models.iter().all(|model| {
            let Some(endpoint) = &model.endpoint else {
                return true;
            };
            let Some(index) = endpoint.pod_index(pods) else {
                return false;
            };
            endpoint.valid(pods)
                && model.bearer.is_none()
                && *tls.entry(index).or_insert_with(|| endpoint.tls()) == endpoint.tls()
        });
        let efforts_known = program.models.iter().all(|model| {
            model
                .reasoning_effort
                .as_deref()
                .is_none_or(|effort| ReasoningEffort::from_name(effort).is_some())
        });
        endpoints_agree
            && matches!(
                program.ingress_auth.as_deref(),
                None | Some("bearer" | "none")
            )
            && matches!(program.translation.as_deref(), None | Some("off" | "on"))
            && efforts_known
            && program.lifecycle.as_ref().is_none_or(|lifecycle| {
                lifecycle.wake.len() <= MAX_STEPS
                    && lifecycle.status.len() <= MAX_STEPS
                    && lifecycle.wake.iter().all(|answer| answer.valid(true))
                    && lifecycle.status.iter().all(|answer| answer.valid(false))
            })
            && program.steps.len() <= MAX_STEPS
            && program.cold_starts <= MAX_STEPS
            && pods <= MAX_PODS
            && program
                .pods
                .iter()
                .all(|pod| pod.len() <= MAX_ANSWERS && pod.iter().all(AnswerInput::valid))
            && program.steps.iter().all(|step| match step {
                Step::MarkReady | Step::Scrape { .. } => true,
                Step::Request {
                    pad_to_bytes,
                    headers,
                    ..
                } => {
                    pad_to_bytes.is_none_or(|bytes| bytes <= MAX_PAD_BYTES)
                        && headers.iter().all(|line| !line.contains(['\r', '\n']))
                }
            })
    }

    /// Per pod, the TLS configuration it serves with, when an `https` endpoint names it.
    fn pod_tls(
        program: &Program,
        certificates: Option<&Certificates>,
    ) -> Vec<Option<Arc<ServerConfig>>> {
        let mut served = vec![None; program.pods.len()];
        for endpoint in program
            .models
            .iter()
            .filter_map(|model| model.endpoint.as_ref())
        {
            if let (Some(index), Some(trusted), Some(certificates)) = (
                endpoint.pod_index(program.pods.len()),
                endpoint.tls(),
                certificates,
            ) {
                served[index] = Some(Arc::clone(if trusted {
                    &certificates.trusted
                } else {
                    &certificates.untrusted
                }));
            }
        }
        served
    }

    fn taken(list: &Mutex<Vec<String>>) -> Vec<String> {
        list.lock().map(|seen| seen.clone()).unwrap_or_default()
    }

    fn run(program_json: &str) -> Value {
        let mut facts = json!({
            "valid_program": false, "error_code": null, "results": [], "bodies": [],
            "headers": [], "retry_after": [], "arrivals": [], "upstream": [], "acquired": 0, "invalidated": [],
            "released": 0, "authorizations": [], "secret_upstream": false, "records": [],
            "scrapes": [], "connections": [], "received_heads": [], "upstream_ends": [],
            "lifecycle_asks": []
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
        let needs_tls = program.models.iter().any(|model| {
            model
                .endpoint
                .as_ref()
                .is_some_and(|endpoint| endpoint.tls().is_some())
        });
        let certificates = if needs_tls {
            let Some(certificates) = certificates() else {
                facts["error_code"] = json!("fixture:certificates");
                return facts;
            };
            Some(certificates)
        } else {
            None
        };
        let mut pods = Vec::new();
        let served = pod_tls(&program, certificates.as_ref());
        for (index, (script, tls)) in program.pods.iter().zip(served).enumerate() {
            let Ok(pod) = Pod::start(index, script.clone(), &shared, tls) else {
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
            windows: program
                .models
                .iter()
                .filter_map(|model| Some((model.alias.clone(), model.target_max_model_len?)))
                .collect(),
            current: Mutex::new(0),
            cold: AtomicUsize::new(program.cold_starts),
            lifecycle: program.lifecycle.as_ref().map(|lifecycle| {
                Mutex::new((
                    lifecycle.wake.iter().cloned().collect(),
                    lifecycle.status.iter().cloned().collect(),
                ))
            }),
        });
        let records = Arc::new(Records::default());
        let mut counting = None;
        let root = certificates
            .as_ref()
            .map(|certificates| certificates.root.as_slice());
        match compose(&program, source, &mut counting, Arc::clone(&records), root) {
            Ok(handle) => {
                steps(&program, &handle, &shared, &records, &mut facts);
                handle.shutdown();
            }
            Err(code) => {
                facts["error_code"] = json!(code);
            }
        }
        stop(&shared, pods);
        facts["upstream"] = json!(taken(&shared.upstream));
        facts["authorizations"] = json!(taken(&shared.authorizations));
        facts["received_heads"] = json!(taken(&shared.received_heads));
        facts["connections"] = json!(taken(&shared.connections));
        facts["upstream_ends"] = json!(taken(&shared.ends));
        facts["secret_upstream"] = json!(shared.secret_upstream.load(Ordering::SeqCst));
        if let Some(counting) = counting {
            facts["acquired"] = json!(counting.acquired.load(Ordering::SeqCst));
            facts["invalidated"] = json!(taken(&counting.invalidated));
            facts["released"] = json!(counting.released.load(Ordering::SeqCst));
            facts["lifecycle_asks"] = json!(taken(&counting.lifecycle_asks));
        }
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
        let Some(received) = exchange(addr, raw.as_bytes(), &Progress::default(), None, true)
        else {
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
        let samples: Vec<&str> = text.lines().filter(|line| !line.starts_with('#')).collect();
        let run = run_started.elapsed();
        scrapes.extend(samples.iter().enumerate().map(|(at, line)| {
            format!("{index} {}", histogram(&samples, at, run).unwrap_or_else(|| waited(line, run)))
        }));
        format!("{status} scraped")
    }

    /// A sample of `llmgw_request_duration_seconds` as the `scrapes` fact writes it: a finite
    /// bucket `cumulative` when it is at least the bucket before it and at most the `+Inf`
    /// bucket, `_sum` `bounded` when it is at most `_count` times the run, every other sample of
    /// the family as served. `None` for a sample of any other series.
    fn histogram(samples: &[&str], at: usize, run: Duration) -> Option<String> {
        const BUCKET: &str = "llmgw_request_duration_seconds_bucket{";
        const SUM: &str = "llmgw_request_duration_seconds_sum{";
        const COUNT: &str = "llmgw_request_duration_seconds_count{";
        let line = samples[at];
        let (series, value) = line.rsplit_once(' ')?;
        let number = |text: &str| text.parse::<f64>().ok();
        if series.starts_with(SUM) {
            let labels = &series[SUM.len()..];
            let count = samples
                .iter()
                .find_map(|other| other.strip_prefix(COUNT)?.strip_prefix(labels))
                .and_then(|rest| number(rest.trim()))?;
            let bounded = number(value)
                .is_some_and(|sum| (0.0..=count * run.as_secs_f64()).contains(&sum));
            return Some(if bounded { format!("{series} bounded") } else { line.to_owned() });
        }
        if !series.starts_with(BUCKET) || series.ends_with("le=\"+Inf\"}") {
            return None;
        }
        let model = &series[..series.find(",le=")?];
        let value = number(value)?;
        let bucket = |other: &&str| other.starts_with(model) && other.contains(",le=");
        let before = samples[..at]
            .iter()
            .rev()
            .take_while(bucket)
            .next()
            .and_then(|other| number(other.rsplit_once(' ')?.1))
            .unwrap_or(0.0);
        let infinite = samples
            .iter()
            .find(|other| other.starts_with(&format!("{model},le=\"+Inf\"}} ")))
            .and_then(|other| number(other.rsplit_once(' ')?.1))?;
        Some(if (before..=infinite).contains(&value) {
            format!("{series} cumulative")
        } else {
            line.to_owned()
        })
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
            let (close_after, half_close) = match step {
                Step::Request {
                    close_after_bytes,
                    half_close,
                    ..
                } => (*close_after_bytes, half_close.unwrap_or(true)),
                _ => (None, true),
            };
            let received = request_bytes(&program.secret, step)
                .and_then(|raw| exchange(addr, &raw, &progress, close_after, half_close));
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
