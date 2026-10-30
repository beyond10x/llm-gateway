//! The binary's translator (story:gateway-translation): `llm_gateway`'s [`Translator`] port over
//! llm 0.18.0's ingress decoders, protocol clients (llm-bedrock's among them) and ingress encoders,
//! and a tokio runtime the binary owns.
//!
//! [`Translator::prepare`] runs on the gateway's worker thread before any target is asked for:
//! it decodes the client's body onto llm's neutral turn, binds carried reasoning state only when
//! it is a seal the gateway made for this route ([`crate::seal`]) and refuses every other, checks
//! the turn against the model's capabilities and projects it for the upstream wire once against
//! a placeholder binding, so every refusal llm's projection would make is made before a pod
//! wakes. [`Translation::run`] builds the binding for the acquired target, sends the turn once
//! with llm's client for the upstream wire through [`BlockingModel::on_runtime`], and writes each
//! event to the client on its own wire as it arrives; an answer to a responses client hands it
//! the upstream's reasoning state sealed for this route. The specification is step 5 of the
//! `Relay` command of `spec/domains/gateway.yaml`, and `docs/gateway.md` "Translation" is the
//! published contract.

use crate::{
    bedrock::BedrockRoute,
    seal::{Scope, Sealer},
};
use llm_bedrock::{Api, BedrockClient};
use llm_blocking::{BlockingModel, BlockingSink};
use llm_core::{
    AuthKind, BillingKind, BoxFuture, Cancel, Capabilities, Dispatch, Error, ErrorCode, Id, Item,
    Model, Protocol, Provenance, StreamEvent, TurnOutcome, TurnRequest,
};
use llm_credentials::{
    ResolvedSecret, Secret, SecretError, SecretRef, SecretResolver, SecretVersion,
};
use llm_gateway::{
    AnswerFraming, BasePath, RefusalCode, RelayModel, RelayTarget, TargetScheme, ToolCalling,
    TranslatedAnswer, Translation, TranslationFailure, TranslationRequest, Translator, Wire,
};
use llm_http::{HttpClient, Limits};
use llm_providers::{
    Account, ApiKeyHeader, BaseUrl, Binding, BindingDocument, Endpoint, Provider, ServedModel,
    ServingModel,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::runtime::{Builder, Handle, Runtime};

/// How long opening the connection to the upstream of a translated request may take.
pub const TRANSLATION_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the upstream may take to send its answer's head, and between two reads of it.
pub const TRANSLATION_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How long one translated turn may take in all.
pub const TRANSLATION_TURN_TIMEOUT: Duration = Duration::from_secs(3600);
/// The context window, and so the output limit, of a model that declares none.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;
/// The most output a request to a messages upstream that names no limit is given; it is given
/// half the window it is translated under when that is less.
pub const MESSAGES_DEFAULT_OUTPUT_TOKENS: u64 = 16_384;
/// The reasoning efforts a model is translated under: llm's vocabulary below `max`. llm
/// checks the effort the upstream wire receives, so `max` reaches a chat or responses upstream as
/// `xhigh`, and a messages upstream as `max`, which this list refuses.
pub const REASONING_EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];
/// The effort a chat or responses upstream receives for disabled thinking, admitted there beside
/// [`REASONING_EFFORTS`]; a messages upstream has no such effort.
pub const NO_REASONING_EFFORT: &str = "none";
/// The name the binding's account gives the target's credential; only the resolver below
/// answers it.
const SECRET_REFERENCE: &str = "target-credential";
/// Where the placeholder binding of `prepare` points. Nothing is ever sent there: it exists so
/// llm's projection can be run before a target is acquired.
const PLACEHOLDER_URL: &str = "http://127.0.0.1/v1";
/// The diagnostic of an upstream that failed after the answer started.
const FAILED_AFTER_OUTPUT: &str = "the model target failed after the answer started";

/// The binary's translator. Cheap to share: every request prepares its own translation.
pub struct LlmTranslator {
    engine: Arc<Engine>,
    /// Per-alias capability overrides (the routes document's declared capabilities).
    capabilities: BTreeMap<String, Capabilities>,
    /// The models Amazon Bedrock serves, by alias (story:bedrock-hosted-target): each is sent with
    /// llm's Bedrock client instead of a binding to its relay target.
    bedrock: BTreeMap<String, Arc<BedrockRoute>>,
}

/// What every translation shares: the runtime and HTTP client, the answer identities and the
/// seal key.
struct Engine {
    transport: Mutex<Option<Arc<Transport>>>,
    /// Generated when the translator starts and never written out (`crate::seal`).
    sealer: Arc<Sealer>,
    /// A fixed `created`, for a run whose bytes must be exact; the system clock otherwise.
    created: Option<u64>,
    counter: AtomicU64,
}

/// The multi-thread runtime that drives llm's I/O, and the HTTP client every turn shares.
struct Transport {
    runtime: Option<Runtime>,
    http: HttpClient,
}

impl Drop for Transport {
    fn drop(&mut self) {
        // Never block the dropping thread on the runtime's workers.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl Transport {
    fn start() -> Option<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("llm-gateway-translate")
            .enable_all()
            .build()
            .ok()?;
        let limits = Limits {
            response_headers: TRANSLATION_READ_TIMEOUT,
            idle: TRANSLATION_READ_TIMEOUT,
            total: TRANSLATION_TURN_TIMEOUT,
        };
        let http = HttpClient::with_connect_timeout(limits, TRANSLATION_CONNECT_TIMEOUT).ok()?;
        Some(Self {
            runtime: Some(runtime),
            http,
        })
    }

    fn handle(&self) -> Option<Handle> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.handle().clone())
    }
}

impl Engine {
    /// The transport, started now if it is not yet: a runtime that could not start is tried
    /// again by the next translation rather than refusing the binary's start.
    fn transport(&self) -> Option<Arc<Transport>> {
        let mut transport = self
            .transport
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if transport.is_none() {
            *transport = Transport::start().map(Arc::new);
            if transport.is_none() {
                tracing::error!("the translator's runtime or HTTP client could not start");
            }
        }
        transport.clone()
    }

    /// The answer's identifier on `wire` and its `created`: the clock and a counter.
    fn identity(&self, wire: Wire) -> (String, u64) {
        let created = self.created.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs())
        });
        let number = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let id = match wire {
            Wire::Chat => format!("chatcmpl-{created}-{number}"),
            Wire::Responses => format!("resp_{created}_{number}"),
            Wire::Messages => format!("msg_{created}_{number}"),
        };
        (id, created)
    }
}

impl Default for LlmTranslator {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for LlmTranslator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmTranslator")
            .field("overridden", &self.capabilities.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl LlmTranslator {
    /// A translator with its runtime and HTTP client started now.
    pub fn new() -> Self {
        let engine = Engine {
            transport: Mutex::new(None),
            sealer: Arc::new(Sealer::generate()),
            created: None,
            counter: AtomicU64::new(0),
        };
        drop(engine.transport());
        Self {
            engine: Arc::new(engine),
            capabilities: BTreeMap::new(),
            bedrock: BTreeMap::new(),
        }
    }

    /// Translates `alias` under `capabilities` instead of those its `RelayModel` implies.
    #[must_use]
    pub fn with_capabilities(mut self, alias: &str, capabilities: Capabilities) -> Self {
        self.capabilities.insert(alias.to_owned(), capabilities);
        self
    }

    /// Sends `alias` to Amazon Bedrock over `route` instead of a binding to its relay target.
    #[must_use]
    pub(crate) fn with_bedrock(mut self, alias: String, route: Arc<BedrockRoute>) -> Self {
        self.bedrock.insert(alias, route);
        self
    }

    /// Answers with `created` as every answer's `created` (a run whose bytes must be exact:
    /// the conformance program). Identifiers still count from 1.
    #[must_use]
    pub fn with_created(self, created: u64) -> Self {
        let engine = Engine {
            transport: Mutex::new(self.engine.transport()),
            sealer: Arc::clone(&self.engine.sealer),
            created: Some(created),
            counter: AtomicU64::new(0),
        };
        Self {
            engine: Arc::new(engine),
            capabilities: self.capabilities,
            bedrock: self.bedrock,
        }
    }

    /// The capabilities `model` is translated under for `upstream`: an override, else tools and
    /// tool choice when it parses tool calls, temperature and `top_p`, [`REASONING_EFFORTS`]
    /// (and [`NO_REASONING_EFFORT`] for a chat or responses upstream), and a context window and
    /// output limit of its window, else [`DEFAULT_CONTEXT_WINDOW`].
    fn capabilities(&self, model: &RelayModel, upstream: Wire) -> Capabilities {
        if let Some(declared) = self.capabilities.get(model.alias().as_str()) {
            return declared.clone();
        }
        let window = model.window().unwrap_or(DEFAULT_CONTEXT_WINDOW);
        let tools = model.tool_calling() == ToolCalling::Parsed;
        Capabilities {
            tools,
            tool_choice: tools,
            temperature: true,
            top_p: true,
            reasoning_efforts: REASONING_EFFORTS
                .iter()
                .copied()
                .chain((upstream != Wire::Messages).then_some(NO_REASONING_EFFORT))
                .map(ToOwned::to_owned)
                .collect(),
            context_window: window,
            max_output_tokens: window,
            output_streaming: true,
            input_during_output: false,
        }
    }
}

const fn protocol(wire: Wire) -> Protocol {
    match wire {
        Wire::Chat => Protocol::ChatCompletions,
        Wire::Responses => Protocol::Responses,
        Wire::Messages => Protocol::Messages,
    }
}

fn id(value: &str) -> Result<Id, Error> {
    Id::new(value).map_err(|_| Error::invalid("an identifier is not printable ASCII"))
}

/// The provenance a client's request is read under: its wire, and nothing it could attribute
/// state to. It is never bound to anything.
fn ingress_provenance(wire: Wire, alias: &str) -> Result<Provenance, Error> {
    Ok(Provenance {
        protocol: protocol(wire),
        provider: id("client")?,
        account: id("client")?,
        endpoint: id("ingress")?,
        model: id(alias)?,
        binding_revision: id("ingress")?,
    })
}

/// How a binding reaches its upstream.
struct Reach<'a> {
    base_url: String,
    credential: Credential<'a>,
}

/// What the upstream is sent as its credential.
#[derive(Clone, Copy)]
enum Credential<'a> {
    None,
    /// `authorization: Bearer <credential>`.
    Bearer,
    /// `<header>: <credential>`.
    ApiKey(&'a str),
}

/// The binding for `wire` to the model `alias` served as `upstream_model`, at `reach`.
fn binding(
    wire: Wire,
    alias: &str,
    upstream_model: &str,
    capabilities: Capabilities,
    reach: &Reach<'_>,
) -> Result<Binding, Error> {
    let reference = || {
        SecretRef::new(SECRET_REFERENCE)
            .map_err(|_| Error::invalid("the credential reference is unusable"))
    };
    let (auth_kind, secret_reference_id, api_key_header) = match reach.credential {
        Credential::None => (AuthKind::Anonymous, None, None),
        Credential::Bearer => (AuthKind::Bearer, Some(reference()?), None),
        Credential::ApiKey(header) => (
            AuthKind::ApiKey,
            Some(reference()?),
            Some(ApiKeyHeader::new(header)?),
        ),
    };
    BindingDocument::new(
        Provider {
            id: id("llm-gateway")?,
            category: id("upstream")?,
        },
        Account {
            id: id("target")?,
            provider_id: id("llm-gateway")?,
            auth_kind,
            billing_kind: BillingKind::SelfHosted,
            secret_reference_id,
            api_key_header,
        },
        Endpoint {
            id: id("target")?,
            account_id: id("target")?,
            base_url: BaseUrl::new(&reach.base_url)?,
        },
        ServedModel {
            id: id(alias)?,
            upstream_name: id(upstream_model)?,
        },
        ServingModel {
            id: id(alias)?,
            endpoint_id: id("target")?,
            model_id: id(alias)?,
            protocol: protocol(wire),
            capabilities,
        },
    )
    .bind()
}

/// The refusal a decoding, capability or projection failure is answered with.
const fn refusal(error: &Error) -> RefusalCode {
    match error.code {
        ErrorCode::Unsupported | ErrorCode::TooLarge => RefusalCode::TranslationUnsupported,
        _ => RefusalCode::TranslationInvalid,
    }
}

/// A decoded client request: the turn, and how the client wants its answer.
struct Decoded {
    request: TurnRequest,
    stream: bool,
    include_usage: bool,
}

fn decode(ingress: Wire, alias: &str, body: &[u8]) -> Result<Decoded, Error> {
    match ingress {
        Wire::Chat => {
            let value: Value = serde_json::from_slice(body)
                .map_err(|_| Error::invalid("the request body is not JSON"))?;
            let decoded = llm_chat::decode_ingress_request(&value)?;
            Ok(Decoded {
                request: decoded.request,
                stream: decoded.stream,
                include_usage: decoded.include_usage,
            })
        }
        Wire::Responses => {
            let value: Value = serde_json::from_slice(body)
                .map_err(|_| Error::invalid("the request body is not JSON"))?;
            // The body must name the model the binding answers to: the alias the client named.
            let reading =
                llm_responses::Binding::new(ingress_provenance(ingress, alias)?, id(alias)?);
            Ok(Decoded {
                request: llm_responses::ingest_request(&reading, &value)?,
                stream: true,
                include_usage: false,
            })
        }
        Wire::Messages => {
            let decoded = llm_messages::decode_request(body, &ingress_provenance(ingress, alias)?)?;
            Ok(Decoded {
                request: decoded.request,
                stream: decoded.stream,
                include_usage: false,
            })
        }
    }
}

/// Runs llm's projection for `upstream` once, so every refusal it makes is made before any
/// target is asked for.
fn project(
    upstream: Wire,
    request: &TurnRequest,
    binding: &Binding,
    upstream_model: &str,
) -> Result<(), Error> {
    request.validate_for(binding.provenance(), binding.capabilities())?;
    match upstream {
        Wire::Chat => llm_chat::project_request(request, binding, true).map(drop),
        Wire::Responses => {
            let projection =
                llm_responses::Binding::new(binding.provenance().clone(), id(upstream_model)?);
            llm_responses::project_request(&projection, request).map(drop)
        }
        Wire::Messages => llm_messages::encode_request(request, binding).map(drop),
    }
}

/// Binds the reasoning state a request carries, or refuses the request whole
/// `translation-opaque-state`.
///
/// Only a responses client's `reasoning` item whose `encrypted_content` is a seal this process
/// made for `scope` is carried: the sealed upstream block becomes llm's `Item::Opaque` under
/// `provenance` (rebound to the request's binding before it is sent) for a messages upstream,
/// and a seal over `null`, the only one a chat upstream's reasoning is given, is dropped. Every
/// other carried item, and anything already attributed, is refused. Nothing about the item
/// reaches a log or the refusal.
fn bind_sealed(
    sealer: &Sealer,
    scope: Scope<'_>,
    request: &mut TurnRequest,
    provenance: &Provenance,
) -> Result<(), RefusalCode> {
    let refused = RefusalCode::TranslationOpaqueState;
    let mut items = Vec::with_capacity(request.items.len());
    for item in std::mem::take(&mut request.items) {
        match item {
            Item::UnattributedOpaque { protocol, payload }
                if scope.ingress == Wire::Responses
                    && protocol == Protocol::Responses
                    && payload.get("type").and_then(Value::as_str) == Some("reasoning") =>
            {
                let state = payload
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .and_then(|seal| sealer.open(scope, seal))
                    .ok_or(refused)?;
                match (scope.upstream, state) {
                    (Wire::Chat, Value::Null) => {}
                    (Wire::Messages, state @ Value::Object(_)) => items.push(Item::Opaque {
                        provenance: provenance.clone(),
                        payload: state,
                    }),
                    _ => return Err(refused),
                }
            }
            Item::Opaque { .. } | Item::UnattributedOpaque { .. } => return Err(refused),
            other => items.push(other),
        }
    }
    request.items = items;
    Ok(())
}

/// Attributes every bound reasoning block of `request` to `provenance`: the placeholder binding
/// it is projected against before a target is acquired, then the acquired target's binding.
/// Only [`bind_sealed`] puts one there.
fn rebind(request: &mut TurnRequest, provenance: &Provenance) {
    for item in &mut request.items {
        if let Item::Opaque {
            provenance: bound, ..
        } = item
        {
            bound.clone_from(provenance);
        }
    }
}

/// The provenance a sealed reasoning item is handed to a responses client under: the encoder
/// carries only state of the binding its outcome names, and the item names this gateway's seal.
fn sealed_provenance(alias: &str) -> Result<Provenance, Error> {
    Ok(Provenance {
        protocol: Protocol::Responses,
        provider: id("llm-gateway")?,
        account: id("seal")?,
        endpoint: id("seal")?,
        model: id(alias)?,
        binding_revision: id("seal")?,
    })
}

impl Translator for LlmTranslator {
    fn prepare(
        &self,
        request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        let alias = request.model.alias().as_str();
        if let Some(route) = self.bedrock.get(alias) {
            return self.prepare_bedrock(request, alias, route);
        }
        let upstream_model = request.model.upstream_model().as_str();
        let mut decoded =
            decode(request.ingress, alias, request.body).map_err(|error| refusal(&error))?;
        let scope = Scope {
            alias,
            upstream_model,
            ingress: request.ingress,
            upstream: request.upstream,
        };
        let reading =
            ingress_provenance(request.ingress, alias).map_err(|error| refusal(&error))?;
        bind_sealed(&self.engine.sealer, scope, &mut decoded.request, &reading)?;
        let capabilities = self.capabilities(request.model, request.upstream);
        // A messages upstream needs an output limit, and the whole window leaves no room for
        // the input: a request naming none is given half the window it is translated under
        // (the model's, else the capabilities'), at most MESSAGES_DEFAULT_OUTPUT_TOKENS.
        if request.upstream == Wire::Messages && decoded.request.max_output_tokens.is_none() {
            let window = request
                .model
                .window()
                .unwrap_or(capabilities.context_window);
            decoded.request.max_output_tokens = Some(
                (window / 2)
                    .clamp(1, MESSAGES_DEFAULT_OUTPUT_TOKENS)
                    .min(capabilities.max_output_tokens),
            );
        }
        let placeholder = Reach {
            base_url: PLACEHOLDER_URL.to_owned(),
            credential: Credential::None,
        };
        binding(
            request.upstream,
            alias,
            upstream_model,
            capabilities.clone(),
            &placeholder,
        )
        .and_then(|binding| {
            rebind(&mut decoded.request, binding.provenance());
            project(request.upstream, &decoded.request, &binding, upstream_model)
        })
        .map_err(|error| refusal(&error))?;
        Ok(Box::new(Prepared {
            engine: Arc::clone(&self.engine),
            ingress: request.ingress,
            upstream: request.upstream,
            alias: alias.to_owned(),
            upstream_model: upstream_model.to_owned(),
            capabilities,
            decoded,
            bedrock: None,
        }))
    }
}

/// The name a client wrote `field` under on `ingress`, for the log line that says it was
/// dropped.
const fn field_name(ingress: Wire, field: Dropped) -> &'static str {
    match (field, ingress) {
        (Dropped::Effort, Wire::Chat) => "reasoning_effort",
        (Dropped::Effort, Wire::Responses) => "reasoning.effort",
        (Dropped::Effort, Wire::Messages) => "output_config.effort",
        (Dropped::ParallelToolCalls, Wire::Messages) => "tool_choice.disable_parallel_tool_use",
        (Dropped::ParallelToolCalls, _) => "parallel_tool_calls",
        (Dropped::Thinking, Wire::Messages) => "thinking",
        (Dropped::Thinking, _) => "reasoning",
    }
}

/// A field a Bedrock target does not take.
#[derive(Clone, Copy)]
enum Dropped {
    /// Bedrock has no effort control on either operation.
    Effort,
    /// Converse has no parallel tool call control.
    ParallelToolCalls,
    /// Converse carries thinking only to an Anthropic model.
    Thinking,
}

/// Removes from `request` each field `route` does not take, logging its name, the alias and
/// nothing else: no value, no prompt and no answer text.
fn drop_for_bedrock(ingress: Wire, alias: &str, route: &BedrockRoute, request: &mut TurnRequest) {
    let mut dropped = Vec::new();
    if request.sampling.reasoning_effort.take().is_some() {
        dropped.push(Dropped::Effort);
    }
    if route.api() == Api::Converse {
        if request.controls.parallel_tool_calls.take().is_some() {
            dropped.push(Dropped::ParallelToolCalls);
        }
        if !route.anthropic() && request.controls.thinking.take().is_some() {
            dropped.push(Dropped::Thinking);
        }
    }
    for field in dropped {
        tracing::info!(
            model = alias,
            field = field_name(ingress, field),
            "bedrock target: dropped a field it does not take"
        );
    }
}

/// Decodes a messages client's request to an `InvokeModel` model whose `anthropic_beta` list is
/// not empty, the client's one header line read by llm's decoder, which refuses a malformed one.
/// The names the model allows stay in the turn in the client's order; each other name is logged
/// by name as dropped. A value that is not a header value at all is refused through
/// [`refusal`] as llm's own `invalid` error, as a malformed body is: `translation-invalid`.
fn decode_with_betas(
    request: &TranslationRequest<'_>,
    alias: &str,
    allowed: &[String],
) -> Result<Decoded, RefusalCode> {
    let mut headers = http::HeaderMap::new();
    if let Some(value) = request.anthropic_beta {
        let value = http::HeaderValue::from_str(value).map_err(|_| {
            refusal(&Error::invalid(
                "the anthropic-beta header is not a header value",
            ))
        })?;
        headers.insert(llm_messages::BETA_HEADER, value);
    }
    let origin = ingress_provenance(request.ingress, alias).map_err(|error| refusal(&error))?;
    let mut decoded = llm_messages::decode_request_with_headers(request.body, &headers, &origin)
        .map_err(|error| refusal(&error))?;
    let betas = &mut decoded.request.controls.hints.anthropic_beta;
    betas.retain(|name| {
        let kept = allowed.contains(name);
        if !kept {
            tracing::info!(
                model = alias,
                field = "anthropic-beta",
                beta = name.as_str(),
                "bedrock target: dropped a field it does not take"
            );
        }
        kept
    });
    Ok(Decoded {
        request: decoded.request,
        stream: decoded.stream,
        include_usage: false,
    })
}

impl LlmTranslator {
    /// Prepares a request for a Bedrock model: decoded, stripped of the fields Bedrock does not
    /// take, given an output limit when it names none, its carried state bound to the target,
    /// and checked by llm's Bedrock client ([`llm_bedrock::check_request`]) before anything is
    /// sent. A messages client's signed thinking reaches `InvokeModel` as it was sent; a
    /// responses client's sealed reasoning is opened as for any other upstream. A messages
    /// client's `anthropic-beta` names reach an `InvokeModel` model's body when its
    /// `anthropic_beta` list allows them ([`decode_with_betas`]); a header to any other Bedrock
    /// model is logged once as dropped, unread, and no client header is sent.
    fn prepare_bedrock(
        &self,
        request: &TranslationRequest<'_>,
        alias: &str,
        route: &Arc<BedrockRoute>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        let invoke = route.api() == Api::AnthropicInvoke;
        let carries_betas =
            request.ingress == Wire::Messages && invoke && !route.anthropic_beta.is_empty();
        let mut decoded = if carries_betas {
            decode_with_betas(request, alias, &route.anthropic_beta)?
        } else {
            let decoded =
                decode(request.ingress, alias, request.body).map_err(|error| refusal(&error))?;
            if request.anthropic_beta.is_some() {
                tracing::info!(
                    model = alias,
                    field = "anthropic-beta",
                    "bedrock target: dropped a field it does not take"
                );
            }
            decoded
        };
        drop_for_bedrock(request.ingress, alias, route, &mut decoded.request);
        let target = &route.target;
        let capabilities = target.capabilities.clone();
        if decoded.request.max_output_tokens.is_none() {
            decoded.request.max_output_tokens = Some(
                (capabilities.context_window / 2)
                    .clamp(1, MESSAGES_DEFAULT_OUTPUT_TOKENS)
                    .min(capabilities.max_output_tokens),
            );
        }
        if request.ingress == Wire::Messages && invoke {
            // The client's own thinking blocks, signed by an Anthropic model, go back to one.
            let refused = RefusalCode::TranslationOpaqueState;
            for item in &mut decoded.request.items {
                match item {
                    Item::UnattributedOpaque {
                        protocol: Protocol::Messages,
                        payload,
                    } => {
                        *item = Item::Opaque {
                            provenance: target.provenance.clone(),
                            payload: std::mem::take(payload),
                        };
                    }
                    Item::Opaque { .. } | Item::UnattributedOpaque { .. } => return Err(refused),
                    _ => {}
                }
            }
        } else {
            let scope = Scope {
                alias,
                upstream_model: alias,
                ingress: request.ingress,
                upstream: Wire::Messages,
            };
            bind_sealed(
                &self.engine.sealer,
                scope,
                &mut decoded.request,
                &target.provenance,
            )?;
        }
        alias.clone_into(&mut decoded.request.model);
        decoded
            .request
            .validate_for(&target.provenance, &capabilities)
            .and_then(|()| llm_bedrock::check_request(&decoded.request, target))
            .map_err(|error| refusal(&error))?;
        Ok(Box::new(Prepared {
            engine: Arc::clone(&self.engine),
            ingress: request.ingress,
            upstream: Wire::Messages,
            alias: alias.to_owned(),
            upstream_model: alias.to_owned(),
            capabilities,
            decoded,
            bedrock: Some(Arc::clone(route)),
        }))
    }
}

/// One prepared request.
struct Prepared {
    engine: Arc<Engine>,
    ingress: Wire,
    upstream: Wire,
    alias: String,
    upstream_model: String,
    capabilities: Capabilities,
    decoded: Decoded,
    /// The Bedrock model the turn is sent to with llm's Bedrock client; `None` for a binding to
    /// the acquired target.
    bedrock: Option<Arc<BedrockRoute>>,
}

impl Prepared {
    /// Whether the client is a messages client of an `InvokeModel` target, which takes the
    /// target's signed thinking as it was made.
    fn keeps_thinking(&self) -> bool {
        self.ingress == Wire::Messages
            && self
                .bedrock
                .as_ref()
                .is_some_and(|route| route.api() == Api::AnthropicInvoke)
    }
}

/// Hands llm's client the target's credential, and nothing else. Its copy is overwritten when
/// it is dropped.
struct TargetSecret(Vec<u8>);

impl Drop for TargetSecret {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

impl SecretResolver for TargetSecret {
    fn resolve<'a>(
        &'a self,
        _reference: &'a SecretRef,
    ) -> BoxFuture<'a, Result<ResolvedSecret, SecretError>> {
        Box::pin(async move {
            Ok(ResolvedSecret {
                secret: Secret::new(self.0.clone())?,
                version: SecretVersion::new("target".to_owned())?,
            })
        })
    }
}

/// llm's client for `upstream` over `binding`.
fn client(
    upstream: Wire,
    binding: Binding,
    http: HttpClient,
    resolver: Arc<dyn SecretResolver>,
) -> Result<Arc<dyn Model>, Error> {
    Ok(match upstream {
        Wire::Chat => Arc::new(llm_chat::ChatClient::new(binding, http, resolver)),
        Wire::Responses => Arc::new(llm_responses::ResponsesClient::new(
            binding, http, resolver,
        )?),
        Wire::Messages => Arc::new(llm_messages::MessagesClient::new(binding, http, resolver)?),
    })
}

impl Prepared {
    /// How the acquired target is reached.
    fn reach(target: &dyn RelayTarget) -> Reach<'_> {
        let scheme = match target.scheme() {
            TargetScheme::Http => "http",
            TargetScheme::Https => "https",
        };
        let base = target.base_path().map_or("/v1", BasePath::as_str);
        Reach {
            base_url: format!("{scheme}://{}{base}", target.authority()),
            credential: match (target.bearer(), target.credential_header()) {
                (None, _) => Credential::None,
                (Some(_), None) => Credential::Bearer,
                (Some(_), Some(header)) => Credential::ApiKey(header.as_str()),
            },
        }
    }
}

impl Translation for Prepared {
    fn run(
        mut self: Box<Self>,
        target: &dyn RelayTarget,
        answer: &mut dyn TranslatedAnswer,
    ) -> Result<(), TranslationFailure> {
        let refused = |_: Error| TranslationFailure::Refused(RefusalCode::UpstreamRefused);
        let transport = self
            .engine
            .transport()
            .ok_or(TranslationFailure::Refused(RefusalCode::UpstreamFailed))?;
        let handle = transport
            .handle()
            .ok_or(TranslationFailure::Refused(RefusalCode::UpstreamFailed))?;
        let model: Arc<dyn Model> = if let Some(route) = &self.bedrock {
            // A Bedrock model is reached by its own client and signed with its own credentials:
            // the relay target names nothing it is sent to.
            Arc::new(
                BedrockClient::new(
                    route.target.clone(),
                    transport.http.clone(),
                    Arc::clone(&route.credentials),
                )
                .and_then(|client| client.with_turn_limit(TRANSLATION_TURN_TIMEOUT))
                .map_err(refused)?,
            )
        } else {
            let binding = binding(
                self.upstream,
                &self.alias,
                &self.upstream_model,
                self.capabilities.clone(),
                &Self::reach(target),
            )
            .map_err(refused)?;
            // The bound reasoning blocks belong to the binding the turn is sent over.
            rebind(&mut self.decoded.request, binding.provenance());
            let secret = target
                .bearer()
                .map_or_else(Vec::new, |bearer| bearer.expose().to_vec());
            let resolver: Arc<dyn SecretResolver> = Arc::new(TargetSecret(secret));
            client(self.upstream, binding, transport.http.clone(), resolver).map_err(refused)?
        };
        let blocking = BlockingModel::on_runtime(model, handle).map_err(refused)?;
        let (id, created) = self.engine.identity(self.ingress);
        let keeps_thinking = self.keeps_thinking();
        let encoder = Encoder::new(
            self.ingress,
            &id,
            created,
            self.decoded.include_usage,
            keeps_thinking,
        )
        .map_err(refused)?;
        let cancel = Cancel::new();
        // Only a responses client is handed sealed state (the decision on sealed reasoning
        // state in spec/domains/gateway.yaml).
        let sealing = (self.ingress == Wire::Responses).then(|| Sealing {
            sealer: &self.engine.sealer,
            scope: Scope {
                alias: &self.alias,
                upstream_model: &self.upstream_model,
                ingress: self.ingress,
                upstream: self.upstream,
            },
            answer_id: &id,
        });
        let mut sink = ClientSink {
            encoder,
            answer,
            cancel: cancel.clone(),
            streamed: self.decoded.stream,
            started: false,
            gone: false,
            sealing,
            shown_reasoning: String::new(),
        };
        let result = blocking.turn(&self.decoded.request, &mut sink, &cancel);
        sink.finish(result)
    }
}

/// The client's wire, as llm's ingress encoder for it writes it.
enum Encoder {
    Chat {
        stream: llm_chat::IngressStream,
        id: String,
        created: u64,
        include_usage: bool,
    },
    Responses {
        stream: llm_responses::IngressStream,
        /// The `id` of the reasoning item the stream opened for the reasoning it showed.
        reasoning: Option<String>,
    },
    Messages {
        stream: llm_messages::IngressStream,
        id: String,
        /// A messages client of a Bedrock `InvokeModel` target is handed the signed thinking
        /// the target made, streamed and in the outcome.
        relays_thinking: bool,
    },
}

/// One server-sent event named by its payload's `type`.
fn event(out: &mut Vec<u8>, payload: &Value) {
    let name = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    out.extend_from_slice(b"event: ");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\ndata: ");
    out.extend_from_slice(payload.to_string().as_bytes());
    out.extend_from_slice(b"\n\n");
}

/// One chat chunk.
fn data(out: &mut Vec<u8>, payload: &Value) {
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(payload.to_string().as_bytes());
    out.extend_from_slice(b"\n\n");
}

impl Encoder {
    const fn relays_thinking(&self) -> bool {
        matches!(
            self,
            Self::Messages {
                relays_thinking: true,
                ..
            }
        )
    }

    fn new(
        ingress: Wire,
        id: &str,
        created: u64,
        include_usage: bool,
        relays_thinking: bool,
    ) -> Result<Self, Error> {
        Ok(match ingress {
            Wire::Chat => Self::Chat {
                stream: llm_chat::IngressStream::new(id, created),
                id: id.to_owned(),
                created,
                include_usage,
            },
            Wire::Responses => Self::Responses {
                stream: llm_responses::IngressStream::new(id, created),
                reasoning: None,
            },
            // Only a Bedrock InvokeModel target answers a messages client with thinking it
            // signed; every other translated answer's reasoning could not be signed.
            Wire::Messages => Self::Messages {
                stream: if relays_thinking {
                    llm_messages::IngressStream::new(id, None)?
                } else {
                    llm_messages::IngressStream::new(id, None)?.without_reasoning_relay()
                },
                id: id.to_owned(),
                relays_thinking,
            },
        })
    }

    /// The bytes one event is written as; empty when the wire carries nothing for it.
    fn chunk(&mut self, delta: &StreamEvent) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Chat { stream, .. } => {
                if let Some(payload) = stream.chunk(delta) {
                    data(&mut out, &payload);
                }
            }
            Self::Responses { stream, reasoning } => {
                for payload in stream.chunk(delta) {
                    if reasoning.is_none()
                        && payload["type"] == "response.output_item.added"
                        && payload["item"]["type"] == "reasoning"
                    {
                        *reasoning = payload["item"]["id"].as_str().map(ToOwned::to_owned);
                    }
                    event(&mut out, &payload);
                }
            }
            Self::Messages { stream, .. } => {
                for payload in stream.chunk(delta) {
                    event(&mut out, &payload);
                }
            }
        }
        out
    }

    /// The bytes that end a streamed answer with its outcome.
    fn close(&mut self, outcome: &TurnOutcome) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        match self {
            Self::Chat {
                stream,
                include_usage,
                ..
            } => {
                for payload in stream.close(outcome, *include_usage)? {
                    data(&mut out, &payload);
                }
                out.extend_from_slice(b"data: [DONE]\n\n");
            }
            Self::Responses { stream, .. } => {
                for payload in stream.close(outcome)? {
                    event(&mut out, &payload);
                }
            }
            Self::Messages { stream, .. } => {
                for payload in stream.close(outcome)? {
                    event(&mut out, &payload);
                }
            }
        }
        Ok(out)
    }

    /// The bytes that end a streamed answer with the wire's terminal error: `code` names the
    /// gateway's refusal on chat, and `error` the class on the other two. No diagnostic of the
    /// upstream's is relayed.
    fn fail(&mut self, code: RefusalCode, error: &Error) -> Vec<u8> {
        let mut out = Vec::new();
        let message = if code == RefusalCode::UpstreamFailed {
            FAILED_AFTER_OUTPUT
        } else {
            code.reason()
        };
        let fixed = Error::new(error.code, message).with_retriable(error.retriable);
        match self {
            Self::Chat { .. } => data(
                &mut out,
                &json!({"error": {"code": code.wire(), "message": message}}),
            ),
            Self::Responses { stream, .. } => {
                for mut payload in stream.fail(&fixed) {
                    // llm's encoder writes a fixed message per class; a bound is named instead.
                    if code == RefusalCode::TranslationTooLarge
                        && let Some(failure) = payload.pointer_mut("/response/error/message")
                    {
                        *failure = json!(message);
                    }
                    event(&mut out, &payload);
                }
            }
            Self::Messages { stream, .. } => {
                for payload in stream.fail(&fixed) {
                    event(&mut out, &payload);
                }
            }
        }
        out
    }

    /// A non-streamed answer: the wire's JSON object.
    fn whole(&self, outcome: &TurnOutcome) -> Result<Vec<u8>, Error> {
        let value = match self {
            Self::Chat { id, created, .. } => {
                llm_chat::encode_ingress_completion(outcome, id, *created)?
            }
            Self::Messages { id, .. } => llm_messages::encode_ingress_message(outcome, id)?,
            // Every translated responses request streams.
            Self::Responses { .. } => return Err(Error::unsupported("a responses answer streams")),
        };
        serde_json::to_vec(&value).map_err(|_| Error::invalid("the answer cannot be encoded"))
    }
}

/// Whether a failure is the upstream's answer passing a bound of the translated path: llm's
/// `too-large` once the request was sent (its stream's 65536 events, 33554432 bytes or
/// 1048576-byte line, or an outcome past llm's own bounds).
fn past_a_bound(error: &Error) -> bool {
    error.code == ErrorCode::TooLarge && error.dispatch != Dispatch::NotSent
}

/// What a failure before any output is answered with. A target that could not be reached,
/// timed out, was unavailable, answered `408` or `5xx`, or reset its connection or ended it
/// inside a frame (llm `transport`, `deadline`, `unavailable`) has failed, whatever was
/// dispatched, and is reported to the target source; one whose answer's status refused the
/// request, or whose answer broke its wire (llm `protocol`: a clean end before the terminal
/// event, a call to a tool not offered), has refused it and is kept; an answer past a bound is
/// `translation-too-large`.
fn before_output(error: &Error) -> TranslationFailure {
    if past_a_bound(error) {
        return TranslationFailure::Refused(RefusalCode::TranslationTooLarge);
    }
    match error.code {
        ErrorCode::Transport | ErrorCode::Deadline | ErrorCode::Unavailable => {
            TranslationFailure::UpstreamFailed
        }
        ErrorCode::Protocol
        | ErrorCode::Unauthorized
        | ErrorCode::RateLimited
        | ErrorCode::Refused
        | ErrorCode::InvalidRequest
        | ErrorCode::TooLarge
        | ErrorCode::Unsupported
        | ErrorCode::Cancelled => TranslationFailure::Refused(RefusalCode::UpstreamRefused),
    }
}

/// llm's sink for one turn: each event goes to the client as it arrives, the head written with
/// the first bytes. A write the client does not take cancels the turn.
struct ClientSink<'a> {
    encoder: Encoder,
    answer: &'a mut dyn TranslatedAnswer,
    cancel: Cancel,
    streamed: bool,
    /// The head was written (or tried).
    started: bool,
    /// A write to the client failed.
    gone: bool,
    /// How a responses client is handed the upstream's reasoning state; `None` for the others.
    sealing: Option<Sealing<'a>>,
    /// The reasoning the stream showed a responses client.
    shown_reasoning: String,
}

/// What a responses answer seals its reasoning state with.
struct Sealing<'a> {
    sealer: &'a Sealer,
    scope: Scope<'a>,
    /// The answer's `id`, which the encoder names its items after.
    answer_id: &'a str,
}

impl ClientSink<'_> {
    /// `outcome` as a responses client is handed it: each reasoning block the upstream minted
    /// (llm `Item::Opaque`) in its place as a finished `reasoning` item whose
    /// `encrypted_content` seals the block for this route, the first under the `id` of the item
    /// the streamed reasoning opened and with that reasoning as its summary; and, when a chat
    /// upstream showed reasoning it can mint no state for, that item finished over a seal of
    /// `null`, first. `None` when there is nothing to seal, the client is not a responses
    /// client, or no seal can be made: the outcome is then carried without its blocks.
    fn sealed(&self, outcome: &TurnOutcome) -> Option<TurnOutcome> {
        let (Some(sealing), Encoder::Responses { reasoning, .. }) = (&self.sealing, &self.encoder)
        else {
            return None;
        };
        let opaque = |item: &Item| matches!(item, Item::Opaque { .. });
        let unminted = sealing.scope.upstream == Wire::Chat
            && reasoning.is_some()
            && !outcome.items.iter().any(opaque);
        if !unminted && !outcome.items.iter().any(opaque) {
            return None;
        }
        let provenance = sealed_provenance(sealing.scope.alias).ok()?;
        let mut shown = reasoning.as_deref();
        let mut count = 0_usize;
        let mut sealed_item = |state: &Value| -> Option<Item> {
            let seal = sealing.sealer.seal(sealing.scope, state)?;
            count += 1;
            let (id, summary) = match shown.take() {
                Some(id) if !self.shown_reasoning.is_empty() => (
                    id.to_owned(),
                    json!([{"type": "summary_text", "text": self.shown_reasoning}]),
                ),
                Some(id) => (id.to_owned(), json!([])),
                None => (
                    format!("rs_{}_sealed_{count}", sealing.answer_id),
                    json!([]),
                ),
            };
            Some(Item::Opaque {
                provenance: provenance.clone(),
                payload: json!({
                    "type": "reasoning",
                    "id": id,
                    "summary": summary,
                    "encrypted_content": seal,
                }),
            })
        };
        let mut items = Vec::with_capacity(outcome.items.len() + 1);
        if unminted {
            items.push(sealed_item(&Value::Null)?);
        }
        for item in &outcome.items {
            items.push(match item {
                Item::Opaque { payload, .. } => sealed_item(payload)?,
                other => other.clone(),
            });
        }
        let mut sealed = outcome.clone();
        sealed.items = items;
        sealed.observation.binding = provenance;
        Some(sealed)
    }

    /// Writes `bytes` as the stream's next piece, writing the head first. `false` when the
    /// client did not take them.
    fn write(&mut self, bytes: &[u8]) -> bool {
        if !self.started {
            self.started = true;
            if !self
                .answer
                .head(200, "text/event-stream", AnswerFraming::Stream)
            {
                self.gone = true;
                return false;
            }
        }
        if !self.answer.body(bytes) {
            self.gone = true;
        }
        !self.gone
    }

    /// Ends the answer by what the turn came to.
    fn finish(mut self, result: Result<TurnOutcome, Error>) -> Result<(), TranslationFailure> {
        if self.gone {
            return Ok(());
        }
        match result {
            Ok(mut outcome) => {
                // Continuation state the upstream minted for its own wire reaches a responses
                // client sealed for this route, and no other client: their wires carry no state
                // of another protocol's.
                match self.sealed(&outcome) {
                    Some(sealed) => outcome = sealed,
                    None if self.encoder.relays_thinking() => {}
                    None => outcome
                        .items
                        .retain(|item| !matches!(item, Item::Opaque { .. })),
                }
                self.answered(&outcome)
            }
            Err(error) if self.started => {
                let code = if past_a_bound(&error) {
                    RefusalCode::TranslationTooLarge
                } else {
                    RefusalCode::UpstreamFailed
                };
                let bytes = self.encoder.fail(code, &error);
                self.write(&bytes);
                Ok(())
            }
            Err(error) => Err(before_output(&error)),
        }
    }

    fn answered(&mut self, outcome: &TurnOutcome) -> Result<(), TranslationFailure> {
        let unrepresentable = TranslationFailure::Refused(RefusalCode::TranslationUnrepresentable);
        if !self.streamed {
            let bytes = self.encoder.whole(outcome).map_err(|_| unrepresentable)?;
            let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
            if self
                .answer
                .head(200, "application/json", AnswerFraming::Length(length))
            {
                self.answer.body(&bytes);
            }
            return Ok(());
        }
        match self.encoder.close(outcome) {
            Ok(bytes) => {
                self.write(&bytes);
                Ok(())
            }
            Err(error) if self.started => {
                let refused = Error::new(ErrorCode::Unsupported, error.message);
                let bytes = self
                    .encoder
                    .fail(RefusalCode::TranslationUnrepresentable, &refused);
                self.write(&bytes);
                Ok(())
            }
            Err(_) => Err(unrepresentable),
        }
    }
}

impl BlockingSink for ClientSink<'_> {
    fn emit(&mut self, delta: StreamEvent) -> Result<(), Error> {
        if self.gone {
            return Err(Error::cancelled());
        }
        if !self.streamed {
            // Nothing is written until the outcome, so a client that went away is noticed only
            // by asking, between two events.
            if self.answer.gone() {
                self.gone = true;
                self.cancel.cancel();
                return Err(Error::cancelled());
            }
            return Ok(());
        }
        if let (Some(_), StreamEvent::ReasoningDelta { text }) = (&self.sealing, &delta) {
            self.shown_reasoning.push_str(text);
        }
        let bytes = self.encoder.chunk(&delta);
        if bytes.is_empty() || self.write(&bytes) {
            return Ok(());
        }
        // The client is gone: stop the turn, which drops the upstream connection.
        self.cancel.cancel();
        Err(Error::cancelled())
    }
}
