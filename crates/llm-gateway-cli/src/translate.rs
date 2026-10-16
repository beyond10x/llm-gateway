//! The binary's translator (story:gateway-translation): `llm_gateway`'s [`Translator`] port over
//! llm 0.6.0's ingress decoders, protocol clients and ingress encoders, and a tokio runtime the
//! binary owns.
//!
//! [`Translator::prepare`] runs on the gateway's worker thread before any target is asked for:
//! it decodes the client's body onto llm's neutral turn, refuses carried reasoning state, checks
//! the turn against the model's capabilities and projects it for the upstream wire once against
//! a placeholder binding, so every refusal llm's projection would make is made before a pod
//! wakes. [`Translation::run`] builds the binding for the acquired target, sends the turn once
//! with llm's client for the upstream wire through [`BlockingModel::on_runtime`], and writes each
//! event to the client on its own wire as it arrives. The specification is step 5 of the
//! `Relay` command of `spec/domains/gateway.yaml`, and `docs/gateway.md` "Translation" is the
//! published contract.

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
/// The most output a request to a messages upstream that names no limit is given, for a model
/// with a window; it is given half the window when that is less.
pub const MESSAGES_DEFAULT_OUTPUT_TOKENS: u64 = 16_384;
/// The reasoning efforts a model is translated under: llm's vocabulary below `max`.
pub const REASONING_EFFORTS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];
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
}

/// What every translation shares: the runtime and HTTP client, and the answer identities.
struct Engine {
    transport: Mutex<Option<Arc<Transport>>>,
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
            created: None,
            counter: AtomicU64::new(0),
        };
        drop(engine.transport());
        Self {
            engine: Arc::new(engine),
            capabilities: BTreeMap::new(),
        }
    }

    /// Translates `alias` under `capabilities` instead of those its `RelayModel` implies.
    #[must_use]
    pub fn with_capabilities(mut self, alias: &str, capabilities: Capabilities) -> Self {
        self.capabilities.insert(alias.to_owned(), capabilities);
        self
    }

    /// Answers with `created` as every answer's `created` (a run whose bytes must be exact:
    /// the conformance program). Identifiers still count from 1.
    #[must_use]
    pub fn with_created(self, created: u64) -> Self {
        let engine = Engine {
            transport: Mutex::new(self.engine.transport()),
            created: Some(created),
            counter: AtomicU64::new(0),
        };
        Self {
            engine: Arc::new(engine),
            capabilities: self.capabilities,
        }
    }

    /// The capabilities `model` is translated under: an override, else tools and tool choice
    /// when it parses tool calls, temperature and `top_p`, [`REASONING_EFFORTS`], and a context
    /// window and output limit of its window, else [`DEFAULT_CONTEXT_WINDOW`].
    fn capabilities(&self, model: &RelayModel) -> Capabilities {
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
                .map(|&effort| effort.to_owned())
                .collect(),
            context_window: window,
            max_output_tokens: window,
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

/// Refuses an object at `level` that holds a key outside `allowed`; anything else passes, for
/// the decoder to judge. The diagnostic names the level, never the key the client sent.
fn closed(value: &Value, allowed: &[&str], level: &str) -> Result<(), Error> {
    match value.as_object() {
        Some(object) if object.keys().any(|key| !allowed.contains(&key.as_str())) => {
            Err(Error::unsupported(format!(
                "a request {level} carries a key outside the subset"
            )))
        }
        _ => Ok(()),
    }
}

/// Refuses a `type` other than `function` where the decoder reads the object as a function
/// but not its `type`.
fn function_typed(value: &Value, level: &str) -> Result<(), Error> {
    match value.get("type") {
        None => Ok(()),
        Some(kind) if kind == "function" => Ok(()),
        Some(_) => Err(Error::unsupported(format!(
            "a request {level} names a type outside the subset"
        ))),
    }
}

/// The chat subset closed at every level below the top that llm 0.6.0's decoder reads, which
/// passes over any other key there (`crates/llm-chat/src/ingress.rs` `decode_messages`,
/// `content_text`, `decode_tool_call`, `decode_tools`, `decode_tool_choice`; `docs/chat.md`).
/// The top level and `stream_options` the decoder closes itself; a role it does not know it
/// refuses.
fn chat_closed(body: &Value) -> Result<(), Error> {
    for message in body
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let allowed: &[&str] = match message.get("role").and_then(Value::as_str) {
            Some("system" | "developer" | "user") => &["role", "content"],
            Some("assistant") => &["role", "content", "tool_calls"],
            Some("tool") => &["role", "content", "tool_call_id"],
            _ => continue,
        };
        closed(message, allowed, "messages[]")?;
        for part in message
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            closed(part, &["type", "text"], "messages[].content[]")?;
        }
        for call in message
            .get("tool_calls")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            closed(call, &["id", "type", "function"], "messages[].tool_calls[]")?;
            function_typed(call, "messages[].tool_calls[]")?;
            if let Some(function) = call.get("function") {
                closed(
                    function,
                    &["name", "arguments"],
                    "messages[].tool_calls[].function",
                )?;
            }
        }
    }
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        closed(tool, &["type", "function"], "tools[]")?;
        if let Some(function) = tool.get("function") {
            closed(
                function,
                &["name", "description", "parameters"],
                "tools[].function",
            )?;
        }
    }
    if let Some(choice) = body.get("tool_choice").filter(|choice| choice.is_object()) {
        closed(choice, &["type", "function"], "tool_choice")?;
        function_typed(choice, "tool_choice")?;
        if let Some(function) = choice.get("function") {
            closed(function, &["name"], "tool_choice.function")?;
        }
    }
    Ok(())
}

/// The responses subset closed at every level below the top that llm 0.6.0's decoder reads,
/// which passes over any other key there (`crates/llm-responses/src/request.rs`
/// `input_to_item`, `content_text`, `wire_to_tool_choice`). The top level, `reasoning` and
/// `tools[]` the decoder closes itself; a `reasoning` item is refused as carried state, and an
/// item of another type the decoder refuses.
fn responses_closed(body: &Value) -> Result<(), Error> {
    for item in body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = item.get("type").and_then(Value::as_str);
        let allowed: &[&str] = match kind {
            Some("message") => &["type", "role", "content"],
            Some("function_call") => &["type", "call_id", "name", "arguments"],
            Some("function_call_output") => &["type", "call_id", "output"],
            _ => continue,
        };
        closed(item, allowed, "input[]")?;
        if kind == Some("message") {
            for part in item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                closed(part, &["type", "text"], "input[].content[]")?;
            }
        }
    }
    if let Some(choice) = body.get("tool_choice").filter(|choice| choice.is_object()) {
        closed(choice, &["type", "name"], "tool_choice")?;
    }
    Ok(())
}

fn decode(ingress: Wire, alias: &str, body: &[u8]) -> Result<Decoded, Error> {
    match ingress {
        Wire::Chat => {
            let value: Value = serde_json::from_slice(body)
                .map_err(|_| Error::invalid("the request body is not JSON"))?;
            chat_closed(&value)?;
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
            responses_closed(&value)?;
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

impl Translator for LlmTranslator {
    fn prepare(
        &self,
        request: &TranslationRequest<'_>,
    ) -> Result<Box<dyn Translation>, RefusalCode> {
        let alias = request.model.alias().as_str();
        let upstream_model = request.model.upstream_model().as_str();
        let mut decoded =
            decode(request.ingress, alias, request.body).map_err(|error| refusal(&error))?;
        if decoded
            .request
            .items
            .iter()
            .any(|item| matches!(item, Item::Opaque { .. } | Item::UnattributedOpaque { .. }))
        {
            return Err(RefusalCode::TranslationOpaqueState);
        }
        let capabilities = self.capabilities(request.model);
        // A messages upstream needs an output limit, and the whole window leaves no room for
        // the input: a request naming none is given half the window, at most
        // MESSAGES_DEFAULT_OUTPUT_TOKENS; llm's projection gives a model without one the
        // capabilities' limit.
        if request.upstream == Wire::Messages
            && decoded.request.max_output_tokens.is_none()
            && let Some(window) = request.model.window()
        {
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
        .and_then(|binding| project(request.upstream, &decoded.request, &binding, upstream_model))
        .map_err(|error| refusal(&error))?;
        Ok(Box::new(Prepared {
            engine: Arc::clone(&self.engine),
            ingress: request.ingress,
            upstream: request.upstream,
            alias: alias.to_owned(),
            upstream_model: upstream_model.to_owned(),
            capabilities,
            decoded,
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
        self: Box<Self>,
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
        let binding = binding(
            self.upstream,
            &self.alias,
            &self.upstream_model,
            self.capabilities.clone(),
            &Self::reach(target),
        )
        .map_err(refused)?;
        let secret = target
            .bearer()
            .map_or_else(Vec::new, |bearer| bearer.expose().to_vec());
        let resolver: Arc<dyn SecretResolver> = Arc::new(TargetSecret(secret));
        let model =
            client(self.upstream, binding, transport.http.clone(), resolver).map_err(refused)?;
        let blocking = BlockingModel::on_runtime(model, handle).map_err(refused)?;
        let (id, created) = self.engine.identity(self.ingress);
        let encoder = Encoder::new(self.ingress, &id, created, self.decoded.include_usage)
            .map_err(refused)?;
        let cancel = Cancel::new();
        let mut sink = ClientSink {
            encoder,
            answer,
            cancel: cancel.clone(),
            streamed: self.decoded.stream,
            started: false,
            gone: false,
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
    Responses(llm_responses::IngressStream),
    Messages {
        stream: llm_messages::IngressStream,
        id: String,
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
    fn new(ingress: Wire, id: &str, created: u64, include_usage: bool) -> Result<Self, Error> {
        Ok(match ingress {
            Wire::Chat => Self::Chat {
                stream: llm_chat::IngressStream::new(id, created),
                id: id.to_owned(),
                created,
                include_usage,
            },
            Wire::Responses => Self::Responses(llm_responses::IngressStream::new(id, created)),
            // A translated messages answer never comes from a messages upstream, so no
            // reasoning it relays could be signed.
            Wire::Messages => Self::Messages {
                stream: llm_messages::IngressStream::new(id, None)?.without_reasoning_relay(),
                id: id.to_owned(),
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
            Self::Responses(stream) => {
                for payload in stream.chunk(delta) {
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
            Self::Responses(stream) => {
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
            Self::Responses(stream) => {
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
            Self::Responses(_) => return Err(Error::unsupported("a responses answer streams")),
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
/// timed out, was unavailable, answered `408` or `5xx`, or cut or garbled its answer has failed,
/// whatever was dispatched, and is reported to the target source; one whose answer's status
/// refused the request has refused it and is kept; an answer past a bound is
/// `translation-too-large`.
fn before_output(error: &Error) -> TranslationFailure {
    if past_a_bound(error) {
        return TranslationFailure::Refused(RefusalCode::TranslationTooLarge);
    }
    match error.code {
        ErrorCode::Transport
        | ErrorCode::Deadline
        | ErrorCode::Unavailable
        | ErrorCode::Protocol => TranslationFailure::UpstreamFailed,
        ErrorCode::Unauthorized
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
}

impl ClientSink<'_> {
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
                // Continuation state the upstream minted for its own wire cannot be carried to
                // another; the client never replays it to this upstream's protocol.
                outcome
                    .items
                    .retain(|item| !matches!(item, Item::Opaque { .. }));
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
        let bytes = self.encoder.chunk(&delta);
        if bytes.is_empty() || self.write(&bytes) {
            return Ok(());
        }
        // The client is gone: stop the turn, which drops the upstream connection.
        self.cancel.cancel();
        Err(Error::cancelled())
    }
}
