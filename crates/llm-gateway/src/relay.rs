//! Relaying the three model-call wires to a target the embedding selects (story:wire-relay).
//!
//! The gateway reads and checks the request, rewrites `model` (and on the messages wire the
//! reasoning effort, or gives a request naming none the model's declared one), asks the injected
//! [`RelayTargets`] for a target, on the messages wire counts the input at that target and lowers
//! a `max_tokens` the model's window has no room for, sends the request over the connection
//! that target opens, and relays the answer as it arrives. It never opens a
//! connection itself: the target does, so the transport — plain TCP to a loopback fixture, TLS
//! to a pod proxy — and its timeouts belong to the embedding. The specification is the `Relay`
//! command of `spec/domains/gateway.yaml`.

use crate::{
    auth, body,
    error::{
        BasePathError, CredentialHeaderError, RefusalCode, RelayError, StaticHeaderError,
        TargetRefusal, TokenError,
    },
    inventory::Label,
    metrics::{Metrics, NoRecords, UsageRecord, UsageRecords},
    translate::{
        AnswerFraming, TranslatedAnswer, TranslationFailure, TranslationRequest, Translator,
    },
};
use std::{
    any::Any,
    collections::BTreeMap,
    fmt,
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

/// The largest request body the relay reads: 32 MiB, as llmgw (`src/lib.rs:35`).
pub(crate) const MAX_REQUEST_BODY_BYTES: u64 = 33_554_432;
/// The size of one internal read while relaying. Not a limit on anything.
const RELAY_CHUNK: usize = 16_384;
/// Where the output clamp counts a messages request's input at its target (vLLM v0.27.1,
/// `vllm/entrypoints/anthropic/api_router.py` lines 95-133).
const COUNT_TOKENS_PATH: &str = "/v1/messages/count_tokens";

/// A wire the gateway relays. Each has its own path, and the target is asked on the same path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Wire {
    /// `POST /v1/chat/completions`, `OpenAI` chat completions.
    Chat,
    /// `POST /v1/responses`, `OpenAI` responses (the Codex wire).
    Responses,
    /// `POST /v1/messages`, Anthropic messages.
    Messages,
}

impl Wire {
    /// Every wire.
    pub const ALL: [Self; 3] = [Self::Chat, Self::Responses, Self::Messages];

    /// The path the wire is served on, and asked for at the target.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Chat => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::Messages => "/v1/messages",
        }
    }

    /// The wire's name in a deployment document: `chat`, `responses` or `messages`.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }

    pub(crate) fn from_path(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|wire| wire.path() == path)
    }
}

/// One connection to a target: whatever the embedding opened, read and written as bytes.
///
/// It is `Any` so the relay can tell a plain `TcpStream` from any other stream (see
/// [`read_within_timeout`]).
pub trait RelayStream: Read + Write + Send + Any {}

impl<T: Read + Write + Send + 'static> RelayStream for T {}

/// Runs one read of `socket` under the socket's read timeout measured from the first attempt.
///
/// A socket with a read timeout answers a signal, or a stop and continue, with `EINTR`
/// (`man 7 signal`), and the next `read` restarts the whole timeout, so a read retried with no
/// deadline of its own waits for ever on a silent peer while signals keep arriving. Here `read`
/// is retried on `Interrupted` with the socket's read timeout shrunk to what is left of the
/// original; nothing left is `TimedOut`, as a plain timeout is. The original timeout is put
/// back on every path. A socket with no read timeout is retried as before. `read` must read
/// `socket` or a handle on the same connection.
///
/// # Errors
/// What `read` returns, `TimedOut` once the timeout has passed, or the socket's own error when
/// its timeout cannot be read or set.
pub fn read_within_timeout<T>(
    socket: &TcpStream,
    mut read: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let Some(total) = socket.read_timeout()? else {
        loop {
            match read() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                other => return other,
            }
        }
    };
    let start = Instant::now();
    let mut shrunk = false;
    let result = loop {
        match read() {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                let left = total.saturating_sub(start.elapsed());
                if left.is_zero() {
                    break Err(io::ErrorKind::TimedOut.into());
                }
                shrunk = true;
                if let Err(error) = socket.set_read_timeout(Some(left)) {
                    break Err(error);
                }
            }
            other => break other,
        }
    };
    if shrunk {
        let restored = socket.set_read_timeout(Some(total));
        return result.and_then(|value| restored.map(|()| value));
    }
    result
}

/// The credential a target expects as `authorization: Bearer <key>` (row B8): for a Runpod pod,
/// the model's vLLM key. The embedding resolves it and builds this once; the gateway only
/// writes it into the head it sends that target. It follows the owner material's rules, so it
/// is one token wherever it is written. It is redacted in `Debug`, has no `Display` and no
/// `Clone`, and its bytes are overwritten when it is dropped.
pub struct TargetBearer(Vec<u8>);

impl TargetBearer {
    /// Accepts printable US-ASCII credential material of 1..=4096 bytes.
    ///
    /// # Errors
    /// Returns [`TokenError`] for empty, oversized or non-printable material, naming the rule
    /// and never the material.
    pub fn new(material: Vec<u8>) -> Result<Self, TokenError> {
        let bearer = Self(material);
        auth::check_token(&bearer.0)?;
        Ok(bearer)
    }

    /// The material, for an embedding whose own transport sends it (the binary's translator,
    /// story:gateway-translation). The relay writes it into the head it sends and nowhere else;
    /// whoever reads it here holds it to the same rule.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for TargetBearer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TargetBearer([REDACTED])")
    }
}

impl Drop for TargetBearer {
    fn drop(&mut self) {
        // Best effort without a `zeroize` dependency, as `OwnerToken` does.
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// The longest credential header name: `credential-header-bytes`.
const MAX_CREDENTIAL_HEADER_BYTES: usize = 64;

/// The headers a credential header may not be: those the gateway writes into every request it
/// relays, and those that frame or route a message, so a credential can neither replace them
/// nor add a second one.
const RESERVED_HEADERS: [&str; 13] = [
    "host",
    "authorization",
    "content-type",
    "content-length",
    "connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "expect",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
];

/// The header a target expects its credential in, instead of `authorization: Bearer` (an
/// `api-key` hosted endpoint, `llm-gateway.upstream.HostedEndpoint`). It is one RFC 9110 token
/// of at most 64 bytes, held lowercase, and none of the headers the gateway writes itself or
/// that frame a message, so it cannot end its line or add a second framing header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialHeader(String);

impl CredentialHeader {
    /// Accepts 1..=64 RFC 9110 token characters (`credential-header-bytes`) naming no reserved
    /// header, without regard to case.
    ///
    /// # Errors
    /// [`CredentialHeaderError`] naming the rule and never the name.
    pub fn new(name: &str) -> Result<Self, CredentialHeaderError> {
        if name.is_empty() || name.len() > MAX_CREDENTIAL_HEADER_BYTES {
            return Err(CredentialHeaderError::Length);
        }
        let token = |byte: u8| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte);
        if !name.bytes().all(token) {
            return Err(CredentialHeaderError::NotAToken);
        }
        let name = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&name.as_str()) {
            return Err(CredentialHeaderError::Reserved);
        }
        Ok(Self(name))
    }

    /// The name, lowercase, as it is written into the head.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The longest static header value: `static-header-value-bytes`.
const MAX_STATIC_HEADER_VALUE_BYTES: usize = 1024;

/// One static, non-secret request header a target is sent with every request (a hosted
/// endpoint's `headers`, such as `anthropic-version` or `X-Title`). Its name
/// follows a [`CredentialHeader`]'s rule (1..=64 RFC 9110 token characters, held lowercase,
/// none of the headers the gateway writes or that frame a message, `authorization` among them)
/// and its value is 1..=1024 bytes of 0x20-0x7e with no leading or trailing space, so it can
/// neither end its line, nor replace or repeat a framing header, nor carry `authorization`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticHeader {
    name: String,
    value: String,
}

impl StaticHeader {
    /// Accepts a name and a value as the rule above has them.
    ///
    /// # Errors
    /// [`StaticHeaderError`] naming the rule, and never the name or the value.
    pub fn new(name: &str, value: &str) -> Result<Self, StaticHeaderError> {
        let name = CredentialHeader::new(name).map_err(|error| match error {
            CredentialHeaderError::Length => StaticHeaderError::NameLength,
            CredentialHeaderError::NotAToken => StaticHeaderError::NameNotAToken,
            CredentialHeaderError::Reserved => StaticHeaderError::Reserved,
        })?;
        if value.is_empty() || value.len() > MAX_STATIC_HEADER_VALUE_BYTES {
            return Err(StaticHeaderError::ValueLength);
        }
        if !value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
            || value.starts_with(' ')
            || value.ends_with(' ')
        {
            return Err(StaticHeaderError::ValueNotPrintable);
        }
        Ok(Self {
            name: name.0,
            value: value.to_owned(),
        })
    }

    /// The name, lowercase, as it is written into the head.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn value(&self) -> &str {
        &self.value
    }
}

/// The longest base path: `base-path-bytes`.
const MAX_BASE_PATH_BYTES: usize = 256;
/// The most segments a base path holds: `base-path-segments`.
const MAX_BASE_PATH_SEGMENTS: usize = 32;
/// The prefix every wire path starts with, which a base path replaces.
const WIRE_PREFIX: &str = "/v1";

/// The path a hosted endpoint's API lives below (`https://openrouter.ai/api/v1` has `/api/v1`;
/// `llm-gateway.upstream.HostedEndpoint` `base_url`). A request is sent to it joined to the wire
/// path, replacing the wire path's leading `/v1` ([`Self::join`]). It is `/` and 1..=32
/// segments of one or more of `A-Z a-z 0-9 - . _ ~`, none of them `.` or `..`, at most 256
/// bytes, held without a final `/`: so it can neither end the request line it is written into
/// nor address anything outside the prefix it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasePath(String);

impl BasePath {
    /// Accepts a path as the rule above has it; one final `/` is dropped before the rule is
    /// applied, so `/api/v1/` is `/api/v1`.
    ///
    /// # Errors
    /// [`BasePathError`] naming the rule and never the path.
    pub fn new(path: &str) -> Result<Self, BasePathError> {
        let held = path.strip_suffix('/').unwrap_or(path);
        if held.len() > MAX_BASE_PATH_BYTES {
            return Err(BasePathError::Length);
        }
        let Some(segments) = held.strip_prefix('/') else {
            return Err(BasePathError::NotAPath);
        };
        let mut count = 0_usize;
        for segment in segments.split('/') {
            count += 1;
            if count > MAX_BASE_PATH_SEGMENTS {
                return Err(BasePathError::Length);
            }
            let allowed = |byte: u8| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            };
            if segment.is_empty() || !segment.bytes().all(allowed) {
                return Err(BasePathError::NotAPath);
            }
            if segment == "." || segment == ".." {
                return Err(BasePathError::DotSegment);
            }
        }
        Ok(Self(held.to_owned()))
    }

    /// The path, without a final `/`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path a request on `wire_path` is sent to: this path followed by `wire_path` without
    /// its leading `/v1`. `/api/v1` and `/v1/chat/completions` make `/api/v1/chat/completions`;
    /// `/v1` and `/v1/messages` make `/v1/messages`.
    pub fn join(&self, wire_path: &str) -> String {
        let rest = wire_path.strip_prefix(WIRE_PREFIX).unwrap_or(wire_path);
        format!("{}{rest}", self.0)
    }
}

/// How a target is reached by a transport of the embedding's that opens its own connection: a
/// translator's HTTP client (story:gateway-translation). The byte relay never reads it, because
/// [`RelayTarget::connect`] already opens the right connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TargetScheme {
    /// Plain HTTP. The default.
    #[default]
    Http,
    /// HTTPS.
    Https,
}

/// One target, held for as long as a relayed answer streams and released when dropped.
pub trait RelayTarget: Send {
    /// The target's `host[:port]`. It is sent as `host`, and it is what the gateway names back
    /// in [`RelayTargets::invalidate`] when a request through this target fails.
    fn authority(&self) -> &str;

    /// How a translator reaches [`Self::authority`]. [`TargetScheme::Http`], the default.
    fn scheme(&self) -> TargetScheme {
        TargetScheme::Http
    }

    /// The credential this target expects, sent as `authorization: Bearer <key>`. `None`, the
    /// default, sends no `authorization`. The client's own headers are never forwarded, so the
    /// owner's credential never reaches a target.
    fn bearer(&self) -> Option<&TargetBearer> {
        None
    }

    /// The header the [`Self::bearer`] is sent in instead, as `<header>: <key>` with no
    /// `authorization` (an `api-key` hosted endpoint). `None`, the default, sends a bearer as
    /// `authorization: Bearer <key>`. Without a bearer it sends nothing.
    fn credential_header(&self) -> Option<&CredentialHeader> {
        None
    }

    /// The path the target's API lives below. `None`, the default, asks the target on the wire
    /// path itself; a [`BasePath`] asks it on [`BasePath::join`] of the wire path, the output
    /// clamp's count included. `host` is the [`Self::authority`] either way.
    fn base_path(&self) -> Option<&BasePath> {
        None
    }

    /// The static headers the target is sent with every request, the output clamp's count
    /// included, after `host` and in the order given; one named like the target's
    /// [`Self::credential_header`] is skipped. The default sends none.
    fn headers(&self) -> &[StaticHeader] {
        &[]
    }

    /// Whether the source held the request while this target started (rows L6, W6). The gateway
    /// counts such a request in `llmgw_route_cold_start_holds_total`, and the time `acquire`
    /// took in `llmgw_cold_start_wait_seconds_total`. `false`, the default, is a target that
    /// was serving when asked.
    fn held(&self) -> bool {
        false
    }

    /// The profile of the model the target's pod was started on, which the usage record names.
    /// `None`, the default, is a target that is not a pod of a profiled model, or one whose
    /// profile is unknown.
    fn profile(&self) -> Option<String> {
        None
    }

    /// The longest sequence the target's server takes, when the target knows it: a pod of a
    /// profiled model states its profile's `max_model_len`. The output clamp keeps a messages
    /// request within it, ahead of the model's own [`RelayModel::window`]. `None`, the default,
    /// leaves the model's window.
    fn max_model_len(&self) -> Option<u64> {
        None
    }

    /// Opens one connection to the target. Its read and write timeouts are the target's.
    ///
    /// # Errors
    /// The connection's own error; the gateway treats any as a transport failure.
    fn connect(&self) -> io::Result<Box<dyn RelayStream>>;
}

/// Where a model's requests go. The embedding implements this over its pool.
pub trait RelayTargets: Send + Sync {
    /// The target serving `alias` now, starting one when none is live. The embedding may hold
    /// the request while the target starts (row L6); how long, and what a stop does to a held
    /// request, are its own. The gateway holds nothing itself.
    ///
    /// # Errors
    /// [`TargetRefusal::Unavailable`] when none can be had, answered `target-unavailable`;
    /// [`TargetRefusal::ColdStart`] when the target is still starting and the request's hold
    /// budget has passed, answered `model-cold-start` with `retry-after: 30` (row W6);
    /// [`TargetRefusal::SpendCeiling`] when starting the target would pass a spend ceiling,
    /// answered `spend-ceiling` at once, without `retry-after`.
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal>;

    /// A request for `alias` through the target at `authority` failed (row W7): drop that
    /// endpoint if it is still the current one, so the next acquisition starts a replacement.
    fn invalidate(&self, alias: &str, authority: &str);

    /// The owner's wake of `alias` (`POST /v1/models/{alias}/wake`, story:model-wake-and-status):
    /// start the model's target as the first ask of [`Self::acquire`] would, without holding
    /// anything, and report the status after it. While a start is in progress or the target is
    /// ready it starts nothing more. `None`, the default, is a model this source starts nothing
    /// for (a hosted endpoint): the gateway answers `path-unknown`.
    ///
    /// # Errors
    /// The [`TargetRefusal`] `acquire` would have answered, answered with its refusal.
    fn wake(&self, _alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        None
    }

    /// Where `alias`'s target is (`GET /v1/models/{alias}/status`), read without starting
    /// anything. `None`, the default, as for [`Self::wake`].
    fn status(&self, _alias: &str) -> Option<ModelStatus> {
        None
    }
}

/// Where a model's target is, named by what a request for the model would meet
/// (`llm-gateway.gateway.ModelPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModelPhase {
    /// No target is live: a request starts one.
    Stopped,
    /// A request is held while the target starts.
    Starting,
    /// The target was last observed serving: a request is relayed to it.
    Ready,
    /// A previous target owes a stop not yet confirmed; nothing starts meanwhile.
    Stopping,
}

impl ModelPhase {
    /// Every phase, in declaration order.
    pub const ALL: [Self; 4] = [Self::Stopped, Self::Starting, Self::Ready, Self::Stopping];

    /// The code a lifecycle route answers.
    pub const fn code(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Stopping => "stopping",
        }
    }
}

/// What a lifecycle route answers for one model (`llm-gateway.gateway.ModelStatus`): its
/// phase, the provider's own word for the target while it starts, and the profile it was
/// started on. A value the source does not know is `None`, and the answer leaves its key out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub phase: ModelPhase,
    pub sub_phase: Option<String>,
    pub profile: Option<String>,
}

impl ModelStatus {
    /// A status with neither a sub-phase nor a profile.
    pub const fn new(phase: ModelPhase) -> Self {
        Self {
            phase,
            sub_phase: None,
            profile: None,
        }
    }
}

/// Whether a model's server parses tool calls (`llm-gateway.clients.ToolCalling`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolCalling {
    /// The server parses tool calls: a Runpod pod started with `--enable-auto-tool-choice` and
    /// a `--tool-call-parser`.
    Parsed,
    /// It does not, and answers text only. A body offering tools is refused
    /// `tools-not-served` before any target is asked for. The default, so a model that says
    /// nothing never wakes a pod for a request it cannot answer.
    #[default]
    Absent,
}

/// A reasoning effort a model declares (`llm-gateway.deployment.HostedReasoningEffort`): one the
/// served Qwen3.8 chat template accepts. A messages-wire request that names no effort reaches
/// the target with it as `output_config.effort` (`docs/gateway.md` "The relay", step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReasoningEffort {
    Low,
    Medium,
    Xhigh,
}

impl ReasoningEffort {
    /// Every declarable effort.
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::Xhigh];

    /// The effort as the request carries it: `low`, `medium` or `xhigh`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::Xhigh => "xhigh",
        }
    }

    /// The effort `text` names exactly, if it is one of [`Self::ALL`].
    pub fn from_name(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|effort| effort.as_str() == text)
    }
}

/// A model the gateway relays: its alias, the model name its target expects, its wires,
/// whether it serves tool calls, the two lengths the public routes publish (rows R5, R1), and
/// the effort a messages request naming none is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayModel {
    alias: Label,
    upstream_model: Label,
    wires: Vec<Wire>,
    tool_calling: ToolCalling,
    max_model_len: Option<u64>,
    context_window: Option<u64>,
    reasoning_effort: Option<ReasoningEffort>,
    translated_only: bool,
}

impl RelayModel {
    /// Its tool calling is [`ToolCalling::Absent`] until [`Self::with_tool_calling`].
    ///
    /// # Errors
    /// [`RelayError::NoWire`] for an empty wire set, [`RelayError::RepeatedWire`] for a wire
    /// named twice (row K11).
    pub fn new(alias: Label, upstream_model: Label, wires: Vec<Wire>) -> Result<Self, RelayError> {
        if wires.is_empty() {
            return Err(RelayError::NoWire);
        }
        for (index, wire) in wires.iter().enumerate() {
            if wires[..index].contains(wire) {
                return Err(RelayError::RepeatedWire);
            }
        }
        Ok(Self {
            alias,
            upstream_model,
            wires,
            tool_calling: ToolCalling::default(),
            max_model_len: None,
            context_window: None,
            reasoning_effort: None,
            translated_only: false,
        })
    }

    /// Declares that the model's target takes no client wire verbatim: every request is decoded
    /// and re-encoded by the translator, on every wire, its declared wires included. A model
    /// served by a hosted Bedrock target is such a model; without a translator it is refused
    /// `wire-not-served` on every wire.
    #[must_use]
    pub fn with_translated_only(mut self) -> Self {
        self.translated_only = true;
        self
    }

    pub fn translated_only(&self) -> bool {
        self.translated_only
    }

    /// Declares the effort a messages-wire request that names none reaches the target with,
    /// as `output_config.effort`. Undeclared, such a request is relayed without one.
    #[must_use]
    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    pub fn reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.reasoning_effort
    }

    /// Declares whether the model's server parses tool calls.
    #[must_use]
    pub fn with_tool_calling(mut self, tool_calling: ToolCalling) -> Self {
        self.tool_calling = tool_calling;
        self
    }

    /// Declares the longest sequence the model's server takes, which `GET /v1/models` lists as
    /// `max_model_len`. Undeclared, the listing has no such key.
    #[must_use]
    pub fn with_max_model_len(mut self, max_model_len: u64) -> Self {
        self.max_model_len = Some(max_model_len);
        self
    }

    /// Declares the model's context window, which `GET /` tells each client. Undeclared, a
    /// client profile carries no context setting.
    #[must_use]
    pub fn with_context_window(mut self, context_window: u64) -> Self {
        self.context_window = Some(context_window);
        self
    }

    pub fn tool_calling(&self) -> ToolCalling {
        self.tool_calling
    }

    pub fn max_model_len(&self) -> Option<u64> {
        self.max_model_len
    }

    pub fn context_window(&self) -> Option<u64> {
        self.context_window
    }

    /// The window the output clamp keeps a messages request within: the declared
    /// `max_model_len`, else the declared `context_window`, else none, and a model without one
    /// is never counted. A target that states its own [`RelayTarget::max_model_len`] replaces
    /// it for the calls it serves.
    pub fn window(&self) -> Option<u64> {
        self.max_model_len.or(self.context_window)
    }

    pub fn alias(&self) -> &Label {
        &self.alias
    }

    pub fn upstream_model(&self) -> &Label {
        &self.upstream_model
    }

    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }
}

/// The relayed models and the target source, composed once, with the counters its calls feed
/// and the port its usage records leave through.
pub struct Relay {
    models: BTreeMap<String, RelayModel>,
    targets: Arc<dyn RelayTargets>,
    metrics: Arc<Metrics>,
    records: Arc<dyn UsageRecords>,
    /// Where a request on a wire its model does not declare goes; without one it is refused
    /// `wire-not-served` (story:gateway-translation).
    translator: Option<Arc<dyn Translator>>,
}

impl Relay {
    /// Every (model, wire) pair is registered at zero in the relay's own [`Metrics`]; records
    /// go nowhere until [`Self::with_records`].
    ///
    /// # Errors
    /// [`RelayError::DuplicateModel`] for two models with one alias.
    pub fn new(
        models: Vec<RelayModel>,
        targets: Arc<dyn RelayTargets>,
    ) -> Result<Self, RelayError> {
        let mut indexed = BTreeMap::new();
        for model in models {
            if indexed
                .insert(model.alias.as_str().to_string(), model)
                .is_some()
            {
                return Err(RelayError::DuplicateModel);
            }
        }
        let relay = Self {
            models: indexed,
            targets,
            metrics: Arc::new(Metrics::default()),
            records: Arc::new(NoRecords),
            translator: None,
        };
        relay.register();
        Ok(relay)
    }

    /// Hands every request on a wire its model does not declare to `translator`, which
    /// translates it for the model's first declared wire (story:gateway-translation). A model's
    /// series are then registered on every wire, since it is served on every wire.
    #[must_use]
    pub fn with_translator(mut self, translator: Arc<dyn Translator>) -> Self {
        self.translator = Some(translator);
        self.register();
        self
    }

    /// Whether a translator was composed: then every model accepts every wire.
    pub(crate) fn translates(&self) -> bool {
        self.translator.is_some()
    }

    /// Counts into `metrics` instead, registering every (model, wire) pair in it at zero. An
    /// embedding that counts pod events itself shares one [`Metrics`] with the relay this way.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self.register();
        self
    }

    /// Hands every [`UsageRecord`] to `records` (row O3: the binary logs each).
    #[must_use]
    pub fn with_records(mut self, records: Arc<dyn UsageRecords>) -> Self {
        self.records = records;
        self
    }

    fn register(&self) {
        for model in self.models.values() {
            let wires: &[Wire] = if self.translates() {
                &Wire::ALL
            } else {
                &model.wires
            };
            for wire in wires {
                self.metrics.register(model.alias.as_str(), *wire);
            }
        }
    }

    /// Every relayed model, in alias order: what the public routes render (rows R5, R1).
    pub(crate) fn models(&self) -> impl Iterator<Item = &RelayModel> {
        self.models.values()
    }

    pub(crate) fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// The owner's wake of `alias` (`llm-gateway.gateway.LifecycleRoute`): `model-unknown` for
    /// an alias no relayed model has, `path-unknown` for a model the source starts nothing for,
    /// and otherwise what the source answered.
    pub(crate) fn wake(&self, alias: &str) -> Result<ModelStatus, RefusalCode> {
        if !self.models.contains_key(alias) {
            return Err(RefusalCode::ModelUnknown);
        }
        self.targets
            .wake(alias)
            .ok_or(RefusalCode::PathUnknown)?
            .map_err(TargetRefusal::refusal)
    }

    /// The status of `alias`, refused as [`Self::wake`] is.
    pub(crate) fn status(&self, alias: &str) -> Result<ModelStatus, RefusalCode> {
        if !self.models.contains_key(alias) {
            return Err(RefusalCode::ModelUnknown);
        }
        self.targets.status(alias).ok_or(RefusalCode::PathUnknown)
    }

    /// Feeds a record to the counters, then hands it to the embedding.
    pub(crate) fn observe(&self, record: &UsageRecord) {
        self.metrics.record(record);
        self.records.record(record);
    }
}

impl fmt::Debug for Relay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Relay")
            .field("models", &self.models.values().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// How the client frames its request body, decided from the head alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    Length(u64),
    Chunked,
}

/// A relay the head has admitted.
#[derive(Debug, Clone)]
pub(crate) struct Admitted {
    pub(crate) wire: Wire,
    pub(crate) framing: Framing,
    pub(crate) expects_continue: bool,
    /// The client asked in HTTP/1.1 or later, so its answer may be chunked. An HTTP/1.0 client
    /// is sent the answer unframed, ended by closing the connection (RFC 9112 section 6.1).
    pub(crate) chunked_answer: bool,
    /// The values of the client's `anthropic-beta` header lines, handed to a translator and
    /// never forwarded verbatim.
    pub(crate) anthropic_beta: Vec<String>,
}

/// Decides the body framing from the head (row W1).
///
/// # Errors
/// [`RefusalCode::RequestMalformed`] for a framing this gateway does not read, and
/// [`RefusalCode::BodyTooLarge`] for a declared length over the bound.
pub(crate) fn framing(
    content_length: Option<&str>,
    transfer_encoding: Option<&str>,
) -> Result<Framing, RefusalCode> {
    match (content_length, transfer_encoding) {
        (None, None) => Ok(Framing::Length(0)),
        (None, Some(coding)) if coding.eq_ignore_ascii_case("chunked") => Ok(Framing::Chunked),
        (Some(length), None)
            if !length.is_empty() && length.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            match length.parse::<u64>() {
                Ok(length) if length <= MAX_REQUEST_BODY_BYTES => Ok(Framing::Length(length)),
                _ => Err(RefusalCode::BodyTooLarge),
            }
        }
        _ => Err(RefusalCode::RequestMalformed),
    }
}

/// Bytes read from one side of a relay, through a small buffer.
struct Buffered<'a> {
    source: Source<'a>,
    buffer: Vec<u8>,
    start: usize,
}

enum Source<'a> {
    /// The client, read under one deadline for the whole body.
    Client(&'a mut TcpStream, Instant),
    /// The target, read under the connection's own timeouts.
    Target(&'a mut dyn RelayStream),
}

impl Buffered<'_> {
    fn available(&self) -> &[u8] {
        &self.buffer[self.start..]
    }

    /// Reads more. `Ok(false)` is the end of the stream. Bytes already consumed are dropped
    /// first, so the buffer never holds more than what is unread plus one read.
    fn fill(&mut self) -> io::Result<bool> {
        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        let mut chunk = [0_u8; RELAY_CHUNK];
        // A socket with a read timeout answers a signal, or a stop and continue, with `EINTR`
        // whatever `SA_RESTART` says (`man 7 signal`): such a read is retried, never taken for
        // a peer that failed, and a retry of the target's read keeps the timeout it began with.
        let read = loop {
            let read = match &mut self.source {
                Source::Client(stream, deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    stream.set_read_timeout(Some(left))?;
                    stream.read(&mut chunk)
                }
                Source::Target(stream) => {
                    // A plain `TcpStream` is read within its read timeout from the first
                    // attempt. Any other stream keeps today's retry below: the pod's TLS
                    // stream (`crates/llm-gateway-cli/src/tls.rs`) bounds its own socket reads
                    // with the same helper, so it never surfaces `Interrupted`.
                    let any: &mut dyn Any = &mut **stream;
                    if let Some(tcp) = any.downcast_mut::<TcpStream>() {
                        let tcp = &*tcp;
                        let mut reader = tcp;
                        read_within_timeout(tcp, || reader.read(&mut chunk))
                    } else {
                        stream.read(&mut chunk)
                    }
                }
            };
            match read {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                read => break read?,
            }
        };
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(read > 0)
    }

    /// Up to `limit` buffered bytes, reading first when none are buffered.
    fn take(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        if self.available().is_empty() && !self.fill()? {
            return Ok(Vec::new());
        }
        let count = limit.min(self.available().len());
        let taken = self.available()[..count].to_vec();
        self.start += count;
        Ok(taken)
    }

    /// One line without its CRLF, of at most `limit` bytes. A longer line is `FileTooLarge`, one
    /// that is not UTF-8 is `InvalidData`, and a stream that ends first is `UnexpectedEof`.
    fn line(&mut self, limit: usize) -> io::Result<String> {
        loop {
            if let Some(end) = self.available().windows(2).position(|pair| pair == b"\r\n") {
                let line = String::from_utf8(self.available()[..end].to_vec())
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
                self.start += end + 2;
                return Ok(line);
            }
            if self.available().len() >= limit + 2 {
                return Err(io::ErrorKind::FileTooLarge.into());
            }
            if !self.fill()? {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
        }
    }

    fn expect_crlf(&mut self) -> io::Result<()> {
        if self.line(0)?.is_empty() {
            Ok(())
        } else {
            Err(io::ErrorKind::InvalidData.into())
        }
    }
}

/// A chunk-size line (RFC 9112 section 7.1): `1*HEXDIG`, then nothing or an ignored chunk
/// extension after optional whitespace and `;`. A size past `u64` reads as `u64::MAX`, which
/// every bound refuses.
fn chunk_size(line: &str) -> io::Result<u64> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let end = line
        .bytes()
        .position(|byte| !byte.is_ascii_hexdigit())
        .unwrap_or(line.len());
    let (digits, rest) = line.split_at(end);
    // Whitespace is allowed only before the `;` of an extension (BWS), never alone.
    let extension = rest.trim_start_matches([' ', '\t']);
    if digits.is_empty() || !(rest.is_empty() || extension.starts_with(';')) {
        return Err(invalid());
    }
    let significant = digits.trim_start_matches('0');
    if significant.len() > 16 {
        return Ok(u64::MAX);
    }
    if significant.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(significant, 16).map_err(|_| invalid())
}

/// Why a client body could not be read: framing that is not HTTP is `request-malformed`; a body
/// that ended or stalled before it was complete is `body-incomplete`.
fn body_refusal(error: &io::Error) -> RefusalCode {
    match error.kind() {
        io::ErrorKind::InvalidData | io::ErrorKind::FileTooLarge => RefusalCode::RequestMalformed,
        _ => RefusalCode::BodyIncomplete,
    }
}

/// Reads trailer lines up to the empty line, all of them within `budget` bytes, CRLFs included.
/// They are discarded; the budget is what keeps a trailer section from being unbounded.
/// `Ok(false)` is a section past the budget.
fn skip_trailers(input: &mut Buffered<'_>, budget: usize) -> io::Result<bool> {
    let mut used = 0_usize;
    loop {
        let left = budget.saturating_sub(used);
        if left < 2 {
            return Ok(false);
        }
        let line = match input.line(left - 2) {
            Err(error) if error.kind() == io::ErrorKind::FileTooLarge => return Ok(false),
            other => other?,
        };
        used += line.len() + 2;
        if line.is_empty() {
            return Ok(true);
        }
    }
}

/// Reads the whole client body under one deadline, refusing it the moment it passes the bound.
/// The trailer section of a chunked body counts against the head bound (`request-too-large`).
fn read_body(
    input: &mut Buffered<'_>,
    framing: Framing,
    line_limit: usize,
) -> Result<Vec<u8>, RefusalCode> {
    let malformed = |error: io::Error| body_refusal(&error);
    let mut body = Vec::new();
    match framing {
        Framing::Length(length) => {
            let length = usize::try_from(length).map_err(|_| RefusalCode::BodyTooLarge)?;
            while body.len() < length {
                let piece = input.take(length - body.len()).map_err(malformed)?;
                if piece.is_empty() {
                    return Err(RefusalCode::BodyIncomplete);
                }
                body.extend_from_slice(&piece);
            }
        }
        Framing::Chunked => loop {
            let size =
                chunk_size(&input.line(line_limit).map_err(malformed)?).map_err(malformed)?;
            if size == 0 {
                if !skip_trailers(input, line_limit).map_err(malformed)? {
                    return Err(RefusalCode::RequestTooLarge);
                }
                break;
            }
            let total = u64::try_from(body.len()).unwrap_or(u64::MAX);
            if size > MAX_REQUEST_BODY_BYTES - total {
                return Err(RefusalCode::BodyTooLarge);
            }
            let size = usize::try_from(size).map_err(|_| RefusalCode::BodyTooLarge)?;
            let mut left = size;
            while left > 0 {
                let piece = input.take(left).map_err(malformed)?;
                if piece.is_empty() {
                    return Err(RefusalCode::BodyIncomplete);
                }
                left -= piece.len();
                body.extend_from_slice(&piece);
            }
            input.expect_crlf().map_err(malformed)?;
        },
    }
    Ok(body)
}

/// How a target frames its answer.
#[derive(Debug, Clone, Copy)]
enum Answer {
    Length(u64),
    Chunked { left: u64, started: bool },
    Close,
}

/// The head of a target's answer.
struct Head {
    status: u16,
    content_type: Option<String>,
    framing: Answer,
}

/// Reads the target's answer head. Every head it sends, interim `1xx` heads included, comes out
/// of one `limit`-byte budget, so a target cannot grow memory by sending heads without end.
fn read_answer_head(input: &mut Buffered<'_>, limit: usize) -> io::Result<Head> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let mut used = 0_usize;
    let mut next_line = |input: &mut Buffered<'_>| {
        let left = limit.saturating_sub(used);
        if left < 2 {
            return Err(invalid());
        }
        let line = input.line(left - 2)?;
        used += line.len() + 2;
        Ok(line)
    };
    loop {
        let start = next_line(input)?;
        let mut parts = start.splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        let status = parts.next().unwrap_or_default();
        if !version.starts_with("HTTP/1.") || status.len() != 3 {
            return Err(invalid());
        }
        let status: u16 = status.parse().map_err(|_| invalid())?;
        if !(100..=599).contains(&status) {
            return Err(invalid());
        }
        let mut content_type = None;
        let mut length = None;
        let mut chunked = false;
        loop {
            let line = next_line(input)?;
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or_else(invalid)?;
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "content-type" if value.bytes().all(|byte| (0x20..0x7f).contains(&byte)) => {
                    content_type = Some(value.to_string());
                }
                "content-length" => length = Some(value.parse::<u64>().map_err(|_| invalid())?),
                "transfer-encoding" => chunked = value.to_ascii_lowercase().ends_with("chunked"),
                _ => {}
            }
        }
        // An interim answer is not the answer.
        if status < 200 {
            continue;
        }
        let framing = if chunked {
            Answer::Chunked {
                left: 0,
                started: false,
            }
        } else {
            length.map_or(Answer::Close, Answer::Length)
        };
        return Ok(Head {
            status,
            content_type,
            framing,
        });
    }
}

/// The next piece of a target's decoded body, or `None` at its end.
fn next_piece(
    input: &mut Buffered<'_>,
    framing: &mut Answer,
    line_limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let limit = |left: u64| usize::try_from(left).unwrap_or(usize::MAX).min(RELAY_CHUNK);
    match framing {
        Answer::Length(0) => Ok(None),
        Answer::Length(left) => {
            let piece = input.take(limit(*left))?;
            if piece.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            *left -= u64::try_from(piece.len()).unwrap_or(*left);
            Ok(Some(piece))
        }
        Answer::Chunked { left, started } => {
            if *left == 0 {
                if *started {
                    input.expect_crlf()?;
                }
                *started = true;
                *left = chunk_size(&input.line(line_limit)?)?;
                if *left == 0 {
                    // The target's trailers are bounded like a head, and discarded.
                    if !skip_trailers(input, line_limit)? {
                        return Err(io::ErrorKind::FileTooLarge.into());
                    }
                    *framing = Answer::Length(0);
                    return Ok(None);
                }
            }
            let piece = input.take(limit(*left))?;
            if piece.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            *left -= u64::try_from(piece.len()).unwrap_or(*left);
            Ok(Some(piece))
        }
        Answer::Close => {
            let piece = input.take(RELAY_CHUNK)?;
            Ok((!piece.is_empty()).then_some(piece))
        }
    }
}

/// What a relay needs from the listening surface.
pub(crate) struct Limits {
    pub(crate) line_bytes: usize,
    pub(crate) timeout: Duration,
    /// The output clamp's count deadline, `GatewayConfig::count_timeout`.
    pub(crate) count_timeout: Duration,
}

/// What a relay learned about its call, for the usage record.
#[derive(Debug, Default)]
pub(crate) struct Call {
    /// The relayed model the body named, once it is known.
    pub(crate) model: Option<String>,
    /// The status the target answered with, once its answer head arrived; `None` when no
    /// target answered (llmgw `src/lib.rs:598-629` at 048ebd8).
    pub(crate) target_status: Option<u16>,
    /// Decoded body bytes of the answer written to the client.
    pub(crate) response_bytes: u64,
    /// The profile the acquired target named, once one was acquired.
    pub(crate) profile: Option<String>,
    /// Whether the target's read timed out before its answer head arrived: an
    /// `upstream-failed` that is counted a `timeout` (`llm-gateway.telemetry.Outcome`).
    pub(crate) timed_out: bool,
}

/// A model that serves a request, and the translator when the request is translated.
type Served<'r> = (&'r RelayModel, Option<&'r Arc<dyn Translator>>);

/// The relayed model the body names, if it serves this request (relay steps 4 and 5), and the
/// translator when the request's wire is not one the model declares. Decided before any target
/// is asked for, so a request the model cannot answer never wakes a pod.
fn served_model<'r>(
    relay: &'r Relay,
    analysis: &body::Analysis,
    wire: Wire,
    call: &mut Call,
) -> Result<Served<'r>, RefusalCode> {
    let model = relay
        .models
        .get(&analysis.model)
        .ok_or(RefusalCode::ModelUnknown)?;
    call.model = Some(model.alias.as_str().to_string());
    let translator = if model.wires.contains(&wire) && !model.translated_only {
        None
    } else {
        Some(
            relay
                .translator
                .as_ref()
                .ok_or(RefusalCode::WireNotServed)?,
        )
    };
    if analysis.offers_tools && model.tool_calling == ToolCalling::Absent {
        return Err(RefusalCode::ToolsNotServed);
    }
    Ok((model, translator))
}

/// Acquires the target for `alias`, counting a request held for a starting target on its route
/// with the time it waited (rows O1, O2): the target says it held it, or the source gave up past
/// its hold budget.
fn acquire(
    relay: &Relay,
    alias: &str,
    wire: Wire,
    call: &mut Call,
) -> Result<Box<dyn RelayTarget>, RefusalCode> {
    let asked = Instant::now();
    let acquired = relay.targets.acquire(alias);
    let cold_start = match &acquired {
        Ok(target) => target.held(),
        Err(refusal) => *refusal == TargetRefusal::ColdStart,
    };
    if cold_start {
        relay
            .metrics
            .count_cold_start_hold(alias, wire, asked.elapsed());
    }
    let target = acquired.map_err(TargetRefusal::refusal)?;
    call.profile = target.profile();
    Ok(target)
}

/// The translated path (story:gateway-translation; relay step 5): the translator prepares the
/// request before any target is asked for, then runs it against the acquired target, writing
/// the client's answer itself. The target is released after the last byte.
fn translate(
    stream: &mut TcpStream,
    body: &[u8],
    (model, translator): (&RelayModel, &dyn Translator),
    admitted: &Admitted,
    relay: &Relay,
    limits: &Limits,
    call: &mut Call,
) -> Result<(), RefusalCode> {
    let prepared = translator.prepare(&TranslationRequest {
        ingress: admitted.wire,
        upstream: model.wires[0],
        model,
        body,
        anthropic_beta: &admitted.anthropic_beta,
    })?;
    let alias = model.alias.as_str();
    let target = acquire(relay, alias, admitted.wire, call)?;
    let mut answer = ClientAnswer {
        stream,
        timeout: limits.timeout,
        chunked: admitted.chunked_answer,
        framing: None,
        gone: false,
        written: 0,
    };
    let ran = prepared.run(target.as_ref(), &mut answer);
    let Some(framing) = answer.framing else {
        return match ran {
            // The client went away before anything was written: there is no one to answer.
            Ok(()) if answer.gone => Ok(()),
            Ok(()) => Err(RefusalCode::UpstreamRefused),
            Err(TranslationFailure::UpstreamFailed) => {
                relay.targets.invalidate(alias, target.authority());
                relay.metrics.count_invalidation();
                Err(RefusalCode::UpstreamFailed)
            }
            Err(TranslationFailure::Refused(code)) => Err(code),
        };
    };
    answer.finish(framing);
    call.target_status = answer.status();
    call.response_bytes = answer.written;
    // The target is released only now, after the last byte: row L12's hold.
    drop(target);
    Ok(())
}

/// The relay's [`TranslatedAnswer`] over the client's connection.
struct ClientAnswer<'s> {
    stream: &'s mut TcpStream,
    timeout: Duration,
    /// The client asked in HTTP/1.1 or later, so a stream may be chunked.
    chunked: bool,
    /// The framing the head declared, with its status, once it was written.
    framing: Option<(u16, AnswerFraming)>,
    gone: bool,
    /// Body bytes written to the client.
    written: u64,
}

impl ClientAnswer<'_> {
    fn write(&mut self, bytes: &[u8]) -> bool {
        if self.gone {
            return false;
        }
        if !crate::server::write_by(self.stream, bytes, Instant::now() + self.timeout) {
            self.gone = true;
        }
        !self.gone
    }

    fn status(&self) -> Option<u16> {
        self.framing.map(|(status, _)| status)
    }

    /// Ends a chunked stream with its last chunk, unless the client is gone.
    fn finish(&mut self, framing: (u16, AnswerFraming)) {
        if framing.1 == AnswerFraming::Stream && self.chunked {
            self.write(b"0\r\n\r\n");
        }
    }
}

impl TranslatedAnswer for ClientAnswer<'_> {
    fn head(&mut self, status: u16, content_type: &str, framing: AnswerFraming) -> bool {
        if self.framing.is_some() || self.gone {
            return false;
        }
        self.framing = Some((status, framing));
        let phrase = crate::server::relayed_phrase(status);
        // A content type that could end its line is not written.
        let content_type = if content_type
            .bytes()
            .all(|byte| (0x20..0x7f).contains(&byte))
        {
            format!("content-type: {content_type}\r\n")
        } else {
            String::new()
        };
        let framing = match framing {
            AnswerFraming::Length(length) => format!("content-length: {length}\r\n"),
            AnswerFraming::Stream if self.chunked => "transfer-encoding: chunked\r\n".to_owned(),
            AnswerFraming::Stream => String::new(),
        };
        let head = format!(
            "HTTP/1.1 {status} {phrase}\r\n{content_type}cache-control: no-store\r\nconnection: close\r\n{framing}\r\n"
        );
        self.write(head.as_bytes())
    }

    /// A peek that does not wait: the end of the stream (the client closed it, or its sending
    /// side) or an error is a client gone; bytes waiting, or none yet, a client still there.
    fn gone(&mut self) -> bool {
        if self.gone {
            return true;
        }
        if self.stream.set_nonblocking(true).is_err() {
            return false;
        }
        let mut byte = [0_u8; 1];
        let closed = match self.stream.peek(&mut byte) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error) => !matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ),
        };
        if self.stream.set_nonblocking(false).is_err() {
            return true;
        }
        self.gone |= closed;
        closed
    }

    fn body(&mut self, bytes: &[u8]) -> bool {
        let Some((_, framing)) = self.framing else {
            return false;
        };
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let there = match framing {
            AnswerFraming::Length(length) => {
                if self.written.saturating_add(size) > length {
                    return false;
                }
                self.write(bytes)
            }
            AnswerFraming::Stream if self.chunked => {
                if bytes.is_empty() {
                    return !self.gone;
                }
                let mut framed = format!("{:x}\r\n", bytes.len()).into_bytes();
                framed.extend_from_slice(bytes);
                framed.extend_from_slice(b"\r\n");
                self.write(&framed)
            }
            AnswerFraming::Stream => self.write(bytes),
        };
        if there {
            self.written = self.written.saturating_add(size);
        }
        there
    }
}

/// Relays one admitted request, filling `call` as it learns. A refusal is returned for the
/// caller to write; once the answer's head has been written, nothing more is refused.
pub(crate) fn serve(
    stream: &mut TcpStream,
    leftover: Vec<u8>,
    admitted: &Admitted,
    relay: &Relay,
    limits: &Limits,
    call: &mut Call,
) -> Result<(), RefusalCode> {
    let deadline = Instant::now() + limits.timeout;
    if admitted.expects_continue
        && !crate::server::write_by(stream, b"HTTP/1.1 100 Continue\r\n\r\n", deadline)
    {
        return Err(RefusalCode::RequestMalformed);
    }
    let body = {
        let mut input = Buffered {
            source: Source::Client(stream, deadline),
            buffer: leftover,
            start: 0,
        };
        read_body(&mut input, admitted.framing, limits.line_bytes)?
    };
    let analysis = body::analyse(&body)?;
    let (model, translator) = served_model(relay, &analysis, admitted.wire, call)?;
    if let Some(translator) = translator {
        return translate(
            stream,
            &body,
            (model, translator.as_ref()),
            admitted,
            relay,
            limits,
            call,
        );
    }
    let messages = admitted.wire == Wire::Messages;
    let mut rewritten = analysis.rewritten(
        &body,
        model.upstream_model.as_str(),
        messages,
        model.reasoning_effort.filter(|_| messages),
    );
    drop(body);
    let alias = model.alias.as_str();
    let target = acquire(relay, alias, admitted.wire, call)?;
    // The window of the server that answers: the one the target states, else the model's.
    let window = target
        .max_model_len()
        .or_else(|| model.window())
        .filter(|_| messages);
    // The output clamp (step 7): only now, on the target `acquire` handed out, so the count
    // never wakes a pod the relay would not have woken. A count that fails decides nothing.
    if let (Some(window), Some(_)) = (window, rewritten.max_tokens()) {
        let shared = Arc::new(std::mem::take(&mut rewritten.bytes));
        let counted = count_input(target.as_ref(), &shared, limits);
        // The count's thread lets go of the body before it answers; one abandoned past its
        // deadline may still hold it, and then the relay sends a copy.
        rewritten.bytes = Arc::try_unwrap(shared).unwrap_or_else(|shared| shared.as_ref().clone());
        if let Some(counted) = counted {
            let room = window.saturating_sub(counted);
            if room < 1 {
                return Err(RefusalCode::ContextExceeded);
            }
            rewritten.clamp_max_tokens(room);
        }
    }
    let rewritten = rewritten.bytes;
    let failed = || {
        relay.targets.invalidate(alias, target.authority());
        relay.metrics.count_invalidation();
        RefusalCode::UpstreamFailed
    };
    let mut connection = target.connect().map_err(|_| failed())?;
    let head = request_head(target.as_ref(), admitted.wire.path(), rewritten.len());
    let sent = write_request(connection.as_mut(), &head, &rewritten);
    drop(head);
    drop(rewritten);
    if sent.is_err() {
        return Err(failed());
    }
    let mut input = Buffered {
        source: Source::Target(connection.as_mut()),
        buffer: Vec::new(),
        start: 0,
    };
    let mut head = match read_answer_head(&mut input, limits.line_bytes) {
        Ok(head) => head,
        Err(error) => {
            call.timed_out = matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            );
            return Err(failed());
        }
    };
    call.target_status = Some(head.status);
    // A gateway status from the target means the pod behind it is gone or unreachable; a
    // model error arrives as 4xx or 500 instead (llmgw `src/lib.rs:629-643`).
    if matches!(head.status, 502..=504) {
        return Err(failed());
    }
    call.response_bytes = stream_answer(
        stream,
        &mut input,
        &mut head,
        limits,
        admitted.chunked_answer,
    );
    // The target is released only now, after the last byte: row L12's hold.
    drop(input);
    drop(connection);
    drop(target);
    Ok(())
}

/// A request head, which holds the target's credential when it has one. It is sized before the
/// credential is written into it, so it never reallocates and leaves no copy behind in freed
/// memory, and it is overwritten when dropped, as `TargetBearer` is.
struct RequestHead(Vec<u8>);

impl Drop for RequestHead {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

/// The gateway's own head for `POST <path>` to `target` with a body of `length` bytes. No
/// header of the client's request is forwarded, so the owner's credential never reaches a
/// target. The target's bearer is one printable token (`TargetBearer::new`) and its credential
/// header one token naming no header written here (`CredentialHeader::new`), so neither can end
/// the line it is written into; nor can a static header (`StaticHeader::new`), written after
/// `host` in the order the target gives.
fn request_head(target: &dyn RelayTarget, wire_path: &str, length: usize) -> RequestHead {
    let path = target
        .base_path()
        .map_or_else(|| wire_path.to_owned(), |base| base.join(wire_path));
    let mut start = format!("POST {path} HTTP/1.1\r\nhost: {}\r\n", target.authority());
    // A static header is never named like the target's credential header, written below when
    // the target has a key: the credential wins, and a static header cannot stand in for it.
    let credential_name = target.credential_header().map(CredentialHeader::as_str);
    for header in target.headers() {
        if credential_name == Some(header.name()) {
            continue;
        }
        start.push_str(header.name());
        start.push_str(": ");
        start.push_str(header.value());
        start.push_str("\r\n");
    }
    let end = format!(
        "content-type: application/json\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
    );
    let credential: [&[u8]; 4] = match (target.bearer(), target.credential_header()) {
        (Some(bearer), Some(header)) => [header.as_str().as_bytes(), b": ", &bearer.0, b"\r\n"],
        (Some(bearer), None) => [b"authorization: Bearer ", b"", &bearer.0, b"\r\n"],
        (None, _) => [b""; 4],
    };
    let length =
        start.len() + credential.iter().map(|piece| piece.len()).sum::<usize>() + end.len();
    let mut head = RequestHead(Vec::with_capacity(length));
    let sized = head.0.capacity();
    head.0.extend_from_slice(start.as_bytes());
    for piece in credential {
        head.0.extend_from_slice(piece);
    }
    head.0.extend_from_slice(end.as_bytes());
    debug_assert_eq!(
        head.0.capacity(),
        sized,
        "a head holding a credential reallocated, leaving a copy of it in freed memory"
    );
    head
}

/// Writes `head`, then `body`, to the target.
fn write_request(
    connection: &mut dyn RelayStream,
    head: &RequestHead,
    body: &[u8],
) -> io::Result<()> {
    connection
        .write_all(&head.0)
        .and_then(|()| connection.write_all(body))
        .and_then(|()| connection.flush())
}

/// The output clamp's count: `body`, the request as it is about to be relayed, sent to the same
/// target's `POST /v1/messages/count_tokens` over a connection of its own, and the
/// `input_tokens` of a 2xx answer. `None` for a count that fails: no connection, a request that
/// cannot be written, an answer head that never arrives or is malformed, a status outside 2xx,
/// an answer body longer than the head bound, one without such an `input_tokens`, or an answer
/// not complete within `limits.count_timeout` of the connection opening. A failed count reports
/// nothing to the target source, and is counted in no series.
///
/// The count runs on a thread of its own, so its deadline holds whatever the target's read
/// timeout is: the relay stops waiting at the deadline and the thread, abandoned, ends when the
/// target answers or its own timeout ends the connection. It holds no `RelayTarget`, so an
/// abandoned count keeps no target held.
fn count_input(target: &dyn RelayTarget, body: &Arc<Vec<u8>>, limits: &Limits) -> Option<u64> {
    let mut connection = target.connect().ok()?;
    let opened = Instant::now();
    let head = request_head(target, COUNT_TOKENS_PATH, body.len());
    let body = Arc::clone(body);
    let line_bytes = limits.line_bytes;
    let (answer, answered) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("llm-gateway-count".to_owned())
        .spawn(move || {
            let counted = count_over(connection.as_mut(), &head, &body, line_bytes);
            drop(head);
            drop(body);
            drop(connection);
            let _answered = answer.send(counted);
        })
        .ok()?;
    let left = limits.count_timeout.saturating_sub(opened.elapsed());
    answered.recv_timeout(left).ok().flatten()
}

/// The count's exchange over its open connection: the request, then a 2xx answer's
/// `input_tokens`.
fn count_over(
    connection: &mut dyn RelayStream,
    head: &RequestHead,
    body: &[u8],
    line_bytes: usize,
) -> Option<u64> {
    write_request(connection, head, body).ok()?;
    let mut input = Buffered {
        source: Source::Target(connection),
        buffer: Vec::new(),
        start: 0,
    };
    let mut head = read_answer_head(&mut input, line_bytes).ok()?;
    if !(200..300).contains(&head.status) {
        return None;
    }
    let mut answer = Vec::new();
    while let Some(piece) = next_piece(&mut input, &mut head.framing, line_bytes).ok()? {
        answer.extend_from_slice(&piece);
        if answer.len() > line_bytes {
            return None;
        }
    }
    body::input_tokens(&answer)
}

/// Writes the answer's head, then each decoded piece as it arrives: as one chunk each when
/// `chunked`, so a failure on either side ends the answer without its last chunk and the client
/// sees it cut short; unframed for an HTTP/1.0 client, ended by closing the connection.
/// Returns the decoded body bytes written to the client.
fn stream_answer(
    stream: &mut TcpStream,
    input: &mut Buffered<'_>,
    head: &mut Head,
    limits: &Limits,
    chunked: bool,
) -> u64 {
    let phrase = crate::server::relayed_phrase(head.status);
    let content_type = head
        .content_type
        .as_ref()
        .map_or(String::new(), |value| format!("content-type: {value}\r\n"));
    let framing = if chunked {
        "transfer-encoding: chunked\r\n"
    } else {
        ""
    };
    let start = format!(
        "HTTP/1.1 {} {phrase}\r\n{content_type}cache-control: no-store\r\nconnection: close\r\n{framing}\r\n",
        head.status
    );
    let mut written: u64 = 0;
    if !crate::server::write_by(stream, start.as_bytes(), Instant::now() + limits.timeout) {
        return written;
    }
    loop {
        match next_piece(input, &mut head.framing, limits.line_bytes) {
            Ok(Some(piece)) => {
                let size = u64::try_from(piece.len()).unwrap_or(u64::MAX);
                let framed = if chunked {
                    let mut framed = format!("{:x}\r\n", piece.len()).into_bytes();
                    framed.extend_from_slice(&piece);
                    framed.extend_from_slice(b"\r\n");
                    framed
                } else {
                    piece
                };
                // Each write has its own deadline: a stream may last as long as the model
                // talks, but a client that stops reading cannot hold the connection.
                if !crate::server::write_by(stream, &framed, Instant::now() + limits.timeout) {
                    return written;
                }
                written = written.saturating_add(size);
            }
            Ok(None) => {
                if chunked {
                    crate::server::write_by(stream, b"0\r\n\r\n", Instant::now() + limits.timeout);
                }
                return written;
            }
            Err(_) => return written,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Framing, MAX_REQUEST_BODY_BYTES, Relay, RelayModel, RelayTargets, Wire, framing};
    use crate::{
        error::{
            BasePathError, CredentialHeaderError, RefusalCode, RelayError, StaticHeaderError,
            TargetRefusal,
        },
        inventory::Label,
    };
    use std::sync::Arc;

    struct NoTargets;

    impl RelayTargets for NoTargets {
        fn acquire(&self, _alias: &str) -> Result<Box<dyn super::RelayTarget>, TargetRefusal> {
            Err(TargetRefusal::Unavailable)
        }

        fn invalidate(&self, _alias: &str, _authority: &str) {}
    }

    /// Each way `acquire` can refuse has its own 503 refusal, and no two share one.
    #[test]
    fn every_target_refusal_is_answered_with_its_own_503() {
        for &refusal in TargetRefusal::ALL {
            let expected = match refusal {
                TargetRefusal::Unavailable => RefusalCode::TargetUnavailable,
                TargetRefusal::ColdStart => RefusalCode::ModelColdStart,
                TargetRefusal::SpendCeiling => RefusalCode::SpendCeiling,
            };
            assert_eq!(refusal.refusal(), expected);
            assert_eq!(expected.status(), 503);
        }
    }

    fn model(alias: &str, wires: Vec<Wire>) -> Result<RelayModel, RelayError> {
        RelayModel::new(
            Label::new(alias).unwrap(),
            Label::new("served").unwrap(),
            wires,
        )
    }

    #[test]
    fn every_credential_header_error_has_a_name_that_provokes_it() {
        for expected in CredentialHeaderError::ALL {
            let name = match expected {
                CredentialHeaderError::Length => "k".repeat(65),
                CredentialHeaderError::NotAToken => "x-api key".to_owned(),
                CredentialHeaderError::Reserved => "Transfer-Encoding".to_owned(),
            };
            assert_eq!(
                super::CredentialHeader::new(&name).err(),
                Some(*expected),
                "{name:?}"
            );
        }
    }

    #[test]
    fn every_static_header_error_has_a_header_that_provokes_it() {
        for expected in StaticHeaderError::ALL {
            let (name, value) = match expected {
                StaticHeaderError::NameLength => ("k".repeat(65), "v".to_owned()),
                StaticHeaderError::NameNotAToken => ("x title".to_owned(), "v".to_owned()),
                StaticHeaderError::Reserved => ("Authorization".to_owned(), "v".to_owned()),
                StaticHeaderError::ValueLength => ("x-title".to_owned(), String::new()),
                StaticHeaderError::ValueNotPrintable => {
                    ("x-title".to_owned(), "a\r\nx-injected: b".to_owned())
                }
            };
            assert_eq!(
                super::StaticHeader::new(&name, &value).err(),
                Some(*expected),
                "{name:?}"
            );
        }
        let header = super::StaticHeader::new("X-Title", "b10x gateway").unwrap();
        assert_eq!((header.name(), header.value()), ("x-title", "b10x gateway"));
        for value in [" a", "a ", "a\tb", "a\0b", "a\nb", "caf\u{e9}"] {
            assert_eq!(
                super::StaticHeader::new("x-title", value).err(),
                Some(StaticHeaderError::ValueNotPrintable),
                "{value:?}"
            );
        }
    }

    #[test]
    fn every_base_path_error_has_a_path_that_provokes_it() {
        for expected in BasePathError::ALL {
            let path = match expected {
                BasePathError::Length => format!("/{}", "a".repeat(256)),
                BasePathError::NotAPath => "/api//v1".to_owned(),
                BasePathError::DotSegment => "/api/../admin".to_owned(),
            };
            assert_eq!(
                super::BasePath::new(&path).err(),
                Some(*expected),
                "{path:?}"
            );
        }
    }

    /// A base path replaces the wire path's leading `/v1`; `/v1` itself changes nothing.
    #[test]
    fn a_base_path_replaces_the_v1_prefix_of_every_wire_path() {
        for (base, wire, joined) in [
            (
                "/api/v1",
                "/v1/chat/completions",
                "/api/v1/chat/completions",
            ),
            (
                "/api/v1/",
                "/v1/messages/count_tokens",
                "/api/v1/messages/count_tokens",
            ),
            ("/v1", "/v1/responses", "/v1/responses"),
            (
                "/openai",
                "/v1/chat/completions",
                "/openai/chat/completions",
            ),
            ("/a-b_c.d~e/V2", "/v1/messages", "/a-b_c.d~e/V2/messages"),
        ] {
            assert_eq!(
                super::BasePath::new(base).unwrap().join(wire),
                joined,
                "{base}"
            );
        }
        for path in [
            "",
            "/",
            "//",
            "api/v1",
            "/api/v1//",
            "/a b",
            "/a%2f",
            "/a\\b",
            "/a?b",
            "/a#b",
            "/a\r\nx: y",
            "/./v1",
            "/..",
            "/v1/.",
            "/é",
        ] {
            assert!(super::BasePath::new(path).is_err(), "{path:?}");
        }
        let longest = format!("/{}", "a".repeat(255));
        assert!(super::BasePath::new(&longest).is_ok());
        assert!(super::BasePath::new(&format!("{longest}/")).is_ok());
        assert!(super::BasePath::new(&"/a".repeat(32)).is_ok());
        assert_eq!(
            super::BasePath::new(&"/a".repeat(33)).err(),
            Some(BasePathError::Length)
        );
    }

    #[test]
    fn every_relay_error_has_a_composition_that_provokes_it() {
        for expected in RelayError::ALL {
            let observed = match expected {
                RelayError::NoWire => model("a", Vec::new()).unwrap_err(),
                RelayError::RepeatedWire => {
                    model("a", vec![Wire::Messages, Wire::Messages]).unwrap_err()
                }
                RelayError::DuplicateModel => Relay::new(
                    vec![
                        model("a", vec![Wire::Chat]).unwrap(),
                        model("a", vec![Wire::Responses]).unwrap(),
                    ],
                    Arc::new(NoTargets),
                )
                .unwrap_err(),
            };
            assert_eq!(observed, *expected);
        }
    }

    /// Serves a repeating pattern: one byte first, then reads of `read_size`. With `read_size`
    /// equal to the pattern's period, every read after the first ends one byte into a line, so
    /// the buffer is never fully consumed when it is refilled.
    struct Misaligned {
        pattern: &'static [u8],
        offset: usize,
        read_size: usize,
    }

    impl std::io::Read for Misaligned {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let wanted = if self.offset == 0 { 1 } else { self.read_size };
            let count = wanted.min(buffer.len());
            for byte in &mut buffer[..count] {
                *byte = self.pattern[self.offset % self.pattern.len()];
                self.offset += 1;
            }
            Ok(count)
        }
    }

    impl std::io::Write for Misaligned {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Serves `answer` a few bytes at a time, each read preceded by one that is interrupted, as a
    /// socket with a read timeout is when its thread takes a signal or its process is stopped
    /// and continued (`man 7 signal`): `EINTR` whatever `SA_RESTART` says.
    struct Interrupting {
        answer: &'static [u8],
        offset: usize,
        interrupted: bool,
    }

    impl std::io::Read for Interrupting {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.interrupted = !self.interrupted;
            if self.interrupted {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            let count = buffer.len().min(5).min(self.answer.len() - self.offset);
            buffer[..count].copy_from_slice(&self.answer[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }

    impl std::io::Write for Interrupting {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// An interrupted read of the target's answer is retried, not taken for a target that
    /// failed: under a signal or a stop and continue it answered `upstream-failed` (502).
    #[test]
    fn an_interrupted_read_of_the_answer_is_retried() {
        let mut stream = Interrupting {
            answer:
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
            offset: 0,
            interrupted: false,
        };
        let mut input = super::Buffered {
            source: super::Source::Target(&mut stream),
            buffer: Vec::new(),
            start: 0,
        };
        let head = super::read_answer_head(&mut input, 1_024).unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(input.take(2).unwrap(), b"{}");
    }

    /// The buffer holds what is unread plus one read, however the reads fall across lines: a
    /// buffer that is emptied only when every byte has been consumed grows with everything read.
    #[test]
    fn the_read_buffer_drops_consumed_bytes_whatever_the_read_boundaries() {
        let mut stream = Misaligned {
            pattern: b"1\r\na\r\n",
            offset: 0,
            read_size: 6,
        };
        let mut input = super::Buffered {
            source: super::Source::Target(&mut stream),
            buffer: Vec::new(),
            start: 0,
        };
        for _ in 0..10_000 {
            assert_eq!(input.line(64).unwrap(), "1");
            assert_eq!(input.take(1).unwrap(), b"a");
            input.expect_crlf().unwrap();
            assert!(
                input.buffer.len() <= 12,
                "the buffer holds {} bytes",
                input.buffer.len()
            );
        }
    }

    #[test]
    fn a_chunk_size_is_hex_digits_and_an_optional_extension_only() {
        for (line, size) in [
            ("10", 16),
            ("0", 0),
            ("00000000000000000010", 16),
            ("aF;name=value", 0xaf),
            ("1 ;ext", 1),
            ("1\t;ext", 1),
            ("ffffffffffffffff", u64::MAX),
            ("10000000000000000", u64::MAX),
        ] {
            assert_eq!(super::chunk_size(line).ok(), Some(size), "{line:?}");
        }
        for line in [
            "", "+10", " 10", "-1", "0x10", "10 ", "1 2", "g", "10x", ";ext",
        ] {
            assert!(super::chunk_size(line).is_err(), "{line:?}");
        }
    }

    #[test]
    fn every_wire_is_found_by_its_own_path_and_no_other() {
        for wire in Wire::ALL {
            assert_eq!(Wire::from_path(wire.path()), Some(wire));
        }
        for path in ["/v1/chat", "/v1/messages/", "/v1/routes", "/v1/completions"] {
            assert_eq!(Wire::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn the_body_framing_is_read_from_the_head_alone() {
        assert_eq!(framing(None, None), Ok(Framing::Length(0)));
        assert_eq!(
            framing(Some("33554432"), None),
            Ok(Framing::Length(33_554_432))
        );
        assert_eq!(
            framing(Some("33554433"), None),
            Err(RefusalCode::BodyTooLarge)
        );
        assert_eq!(
            framing(Some("99999999999999999999999"), None),
            Err(RefusalCode::BodyTooLarge)
        );
        assert_eq!(framing(None, Some("Chunked")), Ok(Framing::Chunked));
        for (length, coding) in [
            (Some("-1"), None),
            (Some("1 2"), None),
            (Some(""), None),
            (Some("+5"), None),
            (None, Some("gzip")),
            (None, Some("gzip, chunked")),
            (Some("5"), Some("chunked")),
        ] {
            assert_eq!(
                framing(length, coding),
                Err(RefusalCode::RequestMalformed),
                "{length:?} {coding:?}"
            );
        }
        assert_eq!(MAX_REQUEST_BODY_BYTES, 32 * 1024 * 1024);
    }
}
