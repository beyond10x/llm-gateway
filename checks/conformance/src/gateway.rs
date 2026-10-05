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
pub const VIEWS: &[&str] = &["llm-gateway.gateway.LastExchange"];

const MAX_PROGRAM_BYTES: usize = 64 * 1024;
const MAX_STEPS: usize = 64;
const MAX_PAD_BYTES: usize = 64 * 1024;
/// Twice the crate's route bound, so a fixture can step past it and no further.
const MAX_GENERATED_ROUTES: usize = 8192;
/// How long the client waits for the gateway's answer. Far above any authored read timeout.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Observe one command of this domain, or `None` when the command belongs to another.
pub fn observe(command: &str, input: &Value) -> Option<Result<Observed, TargetError>> {
    if command != "llm-gateway.gateway.Exchange" {
        return None;
    }
    Some(exercise(input))
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
        "headers": [], "secret_on_wire": null, "ready": null, "shutdown": null
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
    facts["secret_on_wire"] = json!(on_wire);
    facts["ready"] = json!(handle.as_ref().map(GatewayHandle::is_ready));
    if let Some(live) = handle {
        live.shutdown();
    }
    facts
}
