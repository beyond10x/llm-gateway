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
    dashboard::{Observed, Progress},
    error::{
        BasePathError, CredentialHeaderError, RefusalCode, RelayError, StaticHeaderError,
        TargetRefusal, TokenError,
    },
    inventory::Label,
    metrics::{Metrics, NoRecords, UsageRecord, UsageRecords},
    translate::{
        AnswerFraming, TranslatedAnswer, TranslationFailure, TranslationRequest, Translator,
    },
    usage::{Usage, UsageScanner},
};
use passthrough::Retry;
pub use passthrough::{
    BodyDecoder, DEFAULT_RETRY_ATTEMPTS, DEFAULT_RETRY_WAIT, Decoded, MAX_ERROR_BODY,
};

mod passthrough;
use std::{
    any::Any,
    collections::BTreeMap,
    fmt,
    io::{self, Read, Write},
    net::{IpAddr, TcpStream},
    sync::{Arc, Mutex, PoisonError, mpsc},
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
/// The model list a newest-model resolution reads.
const MODELS_PATH: &str = "/v1/models?limit=1000";

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
/// [`read_within_timeout`]). An embedding implements it for each stream type it opens; a stream
/// carried over a socket, such as a TLS stream, names that socket in [`Self::shutdown_handle`].
pub trait RelayStream: Read + Write + Send + Any {
    /// A handle on the socket this connection runs over, which the relay shuts down in both
    /// directions when its client leaves while it waits for the answer head: the read of the
    /// head ends at once and the target is released. `None`, the default, is a stream with no
    /// such socket; the relay then waits for the target's answer or its read timeout.
    fn shutdown_handle(&self) -> Option<TcpStream> {
        None
    }
}

impl RelayStream for TcpStream {
    fn shutdown_handle(&self) -> Option<TcpStream> {
        self.try_clone().ok()
    }
}

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

/// The client header a caller names its own conversation in: `OpenCode` Go's
/// `x-opencode-session`, which that provider routes a conversation by and refuses a request
/// without. The relay reads it from the client's head and hands it to the target as
/// [`Caller::session`]; it never forwards it itself (`docs/gateway.md` "The upstream").
pub const CALLER_SESSION_HEADER: &str = "x-opencode-session";
/// The longest caller session a target is told: `caller-session-bytes`.
const MAX_CALLER_SESSION_BYTES: usize = 256;

/// The client headers a model marked [`RelayModel::with_client_credentials`] is never sent,
/// when the gateway authenticates nobody (`ingress_auth = "none"`): the hop-by-hop headers of
/// RFC 9110 section 7.6.1, which belong to the client's connection to the gateway and not to
/// the request, `host`, which names the gateway, `content-length`, which the gateway writes for
/// the body it sends, and `expect`, which the gateway has already answered. Every other header,
/// the client's credential included, is forwarded as the client sent it, and so is every header
/// the client's own `connection` line names as hop-by-hop (story:subscription-passthrough).
pub const UNFORWARDED_HEADERS: [&str; 12] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
    "expect",
];

/// The most client lines one request forwards.
const MAX_CLIENT_LINES: usize = 128;

/// Whether a header `name` belongs to one connection only: an [`UNFORWARDED_HEADERS`] entry, or
/// one the `connection` line names.
fn hop_by_hop(name: &str, connection: &[String]) -> bool {
    UNFORWARDED_HEADERS.contains(&name) || connection.iter().any(|named| named == name)
}

/// The header names a `connection` value lists, in lower case.
fn connection_names(value: Option<&str>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// The forwarded lines of one client request (every header but [`hop_by_hop`] ones), each `name: value\r\n` with the
/// name in lower case and the value as the client sent it, in the order sent, at most
/// [`MAX_CLIENT_LINES`]. Overwritten when dropped: it holds the client's credential.
pub(crate) struct ClientCredentials {
    bytes: Vec<u8>,
    /// Each line's name and the range of its value in `bytes`.
    lines: Vec<(String, std::ops::Range<usize>)>,
}

impl ClientCredentials {
    /// The forwarded lines among `headers`; `None` when the client sent none.
    pub(crate) fn read<'h>(headers: impl Iterator<Item = (&'h str, &'h str)>) -> Option<Self> {
        let headers: Vec<(&str, &str)> = headers.collect();
        let connection = connection_names(
            headers
                .iter()
                .find(|(name, _)| *name == "connection")
                .map(|(_, value)| *value),
        );
        let present: Vec<(&str, &str)> = headers
            .into_iter()
            .filter(|(name, _)| !hop_by_hop(name, &connection))
            .take(MAX_CLIENT_LINES)
            .collect();
        if present.is_empty() {
            return None;
        }
        let length = present
            .iter()
            .map(|(name, value)| name.len() + value.len() + 4)
            .sum();
        let mut credentials = Self {
            bytes: Vec::with_capacity(length),
            lines: Vec::with_capacity(present.len()),
        };
        for (name, value) in present {
            credentials.push(name, value.as_bytes());
        }
        Some(credentials)
    }

    fn push(&mut self, name: &str, value: &[u8]) {
        self.bytes.extend_from_slice(name.as_bytes());
        self.bytes.extend_from_slice(b": ");
        let start = self.bytes.len();
        self.bytes.extend_from_slice(value);
        self.lines.push((name.to_owned(), start..self.bytes.len()));
        self.bytes.extend_from_slice(b"\r\n");
    }

    /// The lines a target is sent: every client line but one the target declares a static
    /// header of its own name for (the route's value wins), the route's `extra` beta values
    /// appended to the client's `anthropic-beta`, distinct, or sent alone when the client
    /// sent none.
    pub(crate) fn forwarded(&self, statics: &[StaticHeader], extra: &[String]) -> Self {
        let overridden = |name: &str| statics.iter().any(|header| header.name() == name);
        let mut extra: Vec<&str> = extra.iter().map(String::as_str).collect();
        let length = self.bytes.len() + extra.iter().map(|beta| beta.len() + 1).sum::<usize>() + 20;
        let mut out = Self {
            bytes: Vec::with_capacity(length),
            lines: Vec::with_capacity(self.lines.len() + 1),
        };
        for (name, range) in &self.lines {
            if overridden(name) {
                continue;
            }
            let value = &self.bytes[range.clone()];
            if name != "anthropic-beta" || extra.is_empty() {
                out.push(name, value);
                continue;
            }
            let sent = String::from_utf8_lossy(value);
            let mut merged = sent.trim().to_owned();
            for beta in std::mem::take(&mut extra) {
                if !sent.split(',').any(|known| known.trim() == beta) {
                    if !merged.is_empty() {
                        merged.push(',');
                    }
                    merged.push_str(beta);
                }
            }
            out.push(name, merged.as_bytes());
        }
        if !extra.is_empty() && !overridden("anthropic-beta") {
            out.push("anthropic-beta", extra.join(",").as_bytes());
        }
        out
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The value of the first line `name`, as sent.
    pub(crate) fn value(&self, name: &str) -> Option<&[u8]> {
        self.lines
            .iter()
            .find(|(line, _)| line == name)
            .map(|(_, range)| &self.bytes[range.clone()])
    }

    /// The lines a model list is asked with: the credential and the `anthropic-*` lines only, so
    /// the list is neither compressed nor typed by a line meant for the call.
    fn for_listing(&self) -> Self {
        let mut out = Self {
            bytes: Vec::with_capacity(self.bytes.len()),
            lines: Vec::new(),
        };
        for (name, range) in &self.lines {
            if matches!(name.as_str(), "authorization" | "x-api-key")
                || name.starts_with("anthropic-")
            {
                out.push(name, &self.bytes[range.clone()]);
            }
        }
        out
    }
}

impl Drop for ClientCredentials {
    fn drop(&mut self) {
        self.bytes.fill(0);
        std::hint::black_box(&self.bytes);
    }
}

impl Clone for ClientCredentials {
    fn clone(&self) -> Self {
        let mut bytes = Vec::with_capacity(self.bytes.len());
        bytes.extend_from_slice(&self.bytes);
        Self {
            bytes,
            lines: self.lines.clone(),
        }
    }
}

impl std::fmt::Debug for ClientCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClientCredentials(<redacted>)")
    }
}

/// Who sent one relayed request, as far as a target may need to know it to choose a
/// per-caller header ([`RelayTarget::caller_header`]): the client's own
/// [`CALLER_SESSION_HEADER`] when it is 1 to 256 bytes of printable ASCII without a space
/// (0x21-0x7e, llm's identifier rule), and the address the client's connection came from. A
/// session that breaks the rule is dropped here, so no target can send or echo it. An IPv4
/// peer reached over an IPv6 socket is its IPv4 address.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caller<'a> {
    session: Option<&'a str>,
    peer: Option<IpAddr>,
}

impl<'a> Caller<'a> {
    /// The caller with `session`, kept only when it follows the rule above, and `peer`.
    pub fn new(session: Option<&'a str>, peer: Option<IpAddr>) -> Self {
        let session = session.filter(|session| {
            (1..=MAX_CALLER_SESSION_BYTES).contains(&session.len())
                && session.bytes().all(|byte| byte.is_ascii_graphic())
        });
        Self {
            session,
            peer: peer.map(|peer| peer.to_canonical()),
        }
    }

    /// The client's own session, when it sent one that follows the rule.
    pub fn session(&self) -> Option<&'a str> {
        self.session
    }

    /// The address the client's connection came from, when it is known.
    pub fn peer(&self) -> Option<IpAddr> {
        self.peer
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
    /// default, sends no `authorization`. The client's own headers are never forwarded (only the
    /// session a target answers in [`RelayTarget::caller_header`]), so the owner's credential
    /// never reaches a target.
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

    /// A header the target is sent for `caller` on this request, in place of the static header
    /// of the same name ([`Self::headers`]), so the target is sent one; the output clamp's count
    /// carries it too. One named like the [`Self::credential_header`] is skipped, as a static
    /// header is. The binary's `OpenCode` Go endpoint answers its `x-opencode-session` here
    /// (`docs/gateway.md` "The upstream"); a translator asks it too. `None`, the default, sends
    /// the static headers as they are.
    fn caller_header(&self, _caller: &Caller<'_>) -> Option<StaticHeader> {
        None
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
    client_credentials: bool,
    model_prefix: Option<String>,
    newest: Option<String>,
    max_tokens_cap: Option<u64>,
    anthropic_beta: Vec<String>,
    retry: Retry,
    compact_window: Option<u64>,
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
            client_credentials: false,
            model_prefix: None,
            newest: None,
            max_tokens_cap: None,
            anthropic_beta: Vec::new(),
            retry: Retry::default(),
            compact_window: None,
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

    /// Declares that the model's target authenticates the client itself: when the gateway
    /// authenticates nobody, the client's headers but [`UNFORWARDED_HEADERS`] are forwarded to it as
    /// sent, and the body reaches it with only its `model` rewritten (no effort mapping, no
    /// output clamp, never translated: a request on a wire it does not declare is
    /// `wire-not-served`). A hosted Anthropic route serving clients on their own subscription is
    /// such a model (story:subscription-passthrough).
    #[must_use]
    pub fn with_client_credentials(mut self) -> Self {
        self.client_credentials = true;
        self
    }

    pub fn client_credentials(&self) -> bool {
        self.client_credentials
    }

    /// Declares that the model serves every model name starting with `prefix` besides its alias,
    /// relaying such a name as the client sent it. The longest matching prefix wins; an alias
    /// matching exactly wins over every prefix.
    ///
    /// # Errors
    /// [`RelayError::EmptyPrefix`] for an empty prefix.
    pub fn with_model_prefix(mut self, prefix: &str) -> Result<Self, RelayError> {
        if prefix.is_empty() {
            return Err(RelayError::EmptyPrefix);
        }
        self.model_prefix = Some(prefix.to_owned());
        Ok(self)
    }

    pub fn model_prefix(&self) -> Option<&str> {
        self.model_prefix.as_deref()
    }

    /// Declares that a request naming the alias is sent the newest model whose ID contains
    /// `family` (`opus`, `sonnet`, …): the first such ID, in the order the target lists them, of
    /// its `GET /v1/models?limit=1000`, asked with the client's own credential lines and
    /// remembered for [`NEWEST_TTL`]. When the list cannot be read the upstream model is sent.
    /// Only a [`Self::with_client_credentials`] model resolves (story:subscription-passthrough).
    ///
    /// # Errors
    /// [`RelayError::EmptyFamily`] for an empty family.
    pub fn with_newest(mut self, family: &str) -> Result<Self, RelayError> {
        if family.is_empty() {
            return Err(RelayError::EmptyFamily);
        }
        self.newest = Some(family.to_ascii_lowercase());
        Ok(self)
    }

    pub fn newest(&self) -> Option<&str> {
        self.newest.as_deref()
    }

    /// Declares the most output tokens a request may ask for: a larger `max_tokens` is lowered
    /// to it, a smaller one and a body without one are sent as they are.
    #[must_use]
    pub fn with_max_tokens_cap(mut self, cap: u64) -> Self {
        self.max_tokens_cap = Some(cap);
        self
    }

    /// Declares `anthropic-beta` values a client-credential model's target is sent besides the
    /// client's own, appended to its line when it does not carry them already.
    ///
    /// # Errors
    /// [`RelayError::BetaNotAToken`] for a value that could not stand in a header list.
    pub fn with_anthropic_beta(mut self, betas: Vec<String>) -> Result<Self, RelayError> {
        let token = |beta: &String| {
            (1..=128).contains(&beta.len())
                && beta
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        };
        if !betas.iter().all(token) {
            return Err(RelayError::BetaNotAToken);
        }
        self.anthropic_beta = betas;
        Ok(self)
    }

    /// Declares how a client-credential model's call is retried on a 429, 502, 503 or 529
    /// answer: at most `max_attempts` attempts (1 is no retry, 0 counts as 1), waiting at most
    /// `max_wait` in all.
    #[must_use]
    pub fn with_retry(mut self, max_attempts: u32, max_wait: Duration) -> Self {
        self.retry = Retry {
            max_attempts: max_attempts.max(1),
            max_wait,
        };
        self
    }

    /// How a client-credential model's call is retried.
    pub fn retry(&self) -> (u32, Duration) {
        (self.retry.max_attempts, self.retry.max_wait)
    }

    /// Declares the auto-compact window a Claude Code session on this model is given by the
    /// launcher (`GET /v1/launch`).
    #[must_use]
    pub fn with_compact_window(mut self, window: u64) -> Self {
        self.compact_window = Some(window);
        self
    }

    /// The auto-compact window, when declared.
    pub fn compact_window(&self) -> Option<u64> {
        self.compact_window
    }

    /// Whether a request on `wire` is relayed to the model byte for byte: it declares the wire
    /// and its target takes client wires verbatim (it is not [`Self::translated_only`]).
    pub(crate) fn relays(&self, wire: Wire) -> bool {
        self.wires.contains(&wire) && !self.translated_only
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
    /// By alias, the wires a model does not relay on whose `GET /` client the translator
    /// admits ([`Translator::admits`]), asked once by [`Self::with_translator`]
    /// (story:client-profiles-translated).
    admitted: BTreeMap<String, Vec<Wire>>,
    /// By alias, the newest model ID a [`RelayModel::with_newest`] model resolved to, and when.
    newest: Mutex<BTreeMap<String, (String, Instant)>>,
    /// Decodes a copy of a content-encoded passthrough answer for its usage and error body.
    decoder: Option<Arc<dyn BodyDecoder>>,
}

impl Relay {
    /// Every (model, wire) pair a model declares is registered at zero in the relay's own
    /// [`Metrics`]; records go nowhere until [`Self::with_records`].
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
            admitted: BTreeMap::new(),
            newest: Mutex::default(),
            decoder: None,
        };
        relay.register();
        Ok(relay)
    }

    /// Hands every request on a wire its model does not declare to `translator`, which
    /// translates it for the model's first declared wire (story:gateway-translation). A model's
    /// route series stay those of the wires it declares: a translated request on another wire
    /// feeds the process-wide series only.
    ///
    /// Asks `translator` now, once per model and wire the model does not relay, whether the
    /// requests of that wire's `GET /` client reach the model ([`Translator::admits`]), so
    /// `GET /` profiles the client there only when they do (story:client-profiles-translated).
    #[must_use]
    pub fn with_translator(mut self, translator: Arc<dyn Translator>) -> Self {
        self.admitted = self
            .models
            .values()
            .map(|model| {
                let upstream = model.wires[0];
                let admitted = Wire::ALL
                    .into_iter()
                    .filter(|&wire| {
                        !model.client_credentials
                            && !model.relays(wire)
                            && translator.admits(wire, model, upstream)
                    })
                    .collect();
                (model.alias.as_str().to_owned(), admitted)
            })
            .collect();
        self.translator = Some(translator);
        self
    }

    /// Whether a translator was composed: then every model accepts every wire.
    pub(crate) fn translates(&self) -> bool {
        self.translator.is_some()
    }

    /// Whether the requests of the client `GET /` profiles on `wire` reach `model`: relayed
    /// ([`RelayModel::relays`]), or translated where the composed translator admitted them.
    pub(crate) fn reaches(&self, model: &RelayModel, wire: Wire) -> bool {
        model.relays(wire)
            || self
                .admitted
                .get(model.alias.as_str())
                .is_some_and(|admitted| admitted.contains(&wire))
    }

    /// Counts into `metrics` instead, registering every declared (model, wire) pair in it at
    /// zero. An embedding that counts pod events itself shares one [`Metrics`] with the relay
    /// this way.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<Metrics>) -> Self {
        self.metrics = metrics;
        self.register();
        self
    }

    /// Reads the usage and the error body of a content-encoded passthrough answer from a copy
    /// `decoder` decodes; the client still receives the answer's bytes as they came. Without
    /// one such an answer's usage stays unknown.
    #[must_use]
    pub fn with_decoder(mut self, decoder: Arc<dyn BodyDecoder>) -> Self {
        self.decoder = Some(decoder);
        self
    }

    pub(crate) fn decoder(&self) -> Option<Arc<dyn BodyDecoder>> {
        self.decoder.clone()
    }

    /// Hands every [`UsageRecord`] to `records` (row O3: the binary logs each).
    #[must_use]
    pub fn with_records(mut self, records: Arc<dyn UsageRecords>) -> Self {
        self.records = records;
        self
    }

    fn register(&self) {
        for model in self.models.values() {
            for wire in &model.wires {
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
    /// The value of the client's `anthropic-beta` header line, handed to a translator and never
    /// forwarded verbatim. The head refuses a repeated header, so a client sends at most one line.
    pub(crate) anthropic_beta: Option<String>,
    /// The value of the client's [`CALLER_SESSION_HEADER`] line, unchecked: [`Caller::new`]
    /// keeps it only when it follows the rule, and it is never forwarded verbatim.
    pub(crate) session: Option<String>,
    /// The client's forwarded lines (every header but [`UNFORWARDED_HEADERS`]), read only when the gateway authenticates
    /// nobody; forwarded only to a model marked [`RelayModel::with_client_credentials`].
    pub(crate) client_credentials: Option<ClientCredentials>,
    /// Who sent the call, for the dashboard: a session hash, a label, the user agent and the
    /// client's retry count; no header value it reads is kept but the user agent.
    pub(crate) observed: Observed,
    /// The request is `POST /v1/messages/count_tokens`: relayed to the same target as a
    /// messages call, at that path, with no output clamp and no token usage recorded.
    pub(crate) count_tokens: bool,
    /// The request target as the client sent it, the path with its query: what a passthrough
    /// route is sent.
    pub(crate) request_target: String,
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
        String::from_utf8(self.bytes_line(limit)?)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
    }

    /// [`Self::line`] without the UTF-8 rule: for a line whose bytes are read only in part.
    fn bytes_line(&mut self, limit: usize) -> io::Result<Vec<u8>> {
        loop {
            if let Some(end) = self.available().windows(2).position(|pair| pair == b"\r\n") {
                let line = self.available()[..end].to_vec();
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
/// every bound refuses. Only the size is read: an extension may carry any byte, since a quoted
/// value may hold obs-text (RFC 9112 section 7.1.1).
fn chunk_size(line: &[u8]) -> io::Result<u64> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let end = line
        .iter()
        .position(|byte| !byte.is_ascii_hexdigit())
        .unwrap_or(line.len());
    let (digits, rest) = line.split_at(end);
    // Whitespace is allowed only before the `;` of an extension (BWS), never alone.
    let blank = rest
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(rest.len());
    if digits.is_empty() || !(rest.is_empty() || rest.get(blank) == Some(&b';')) {
        return Err(invalid());
    }
    let zeros = digits
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(digits.len());
    let significant = &digits[zeros..];
    if significant.len() > 16 {
        return Ok(u64::MAX);
    }
    if significant.is_empty() {
        return Ok(0);
    }
    let significant = std::str::from_utf8(significant).map_err(|_| invalid())?;
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
/// `Ok(false)` is a section past the budget. A client's trailer lines must be UTF-8, as its head
/// must; a target's (`from_target`) may carry any byte, since none is read.
fn skip_trailers(input: &mut Buffered<'_>, budget: usize, from_target: bool) -> io::Result<bool> {
    let mut used = 0_usize;
    loop {
        let left = budget.saturating_sub(used);
        if left < 2 {
            return Ok(false);
        }
        let read = if from_target {
            input.bytes_line(left - 2)
        } else {
            input.line(left - 2).map(String::into_bytes)
        };
        let line = match read {
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
            let size = chunk_size(input.line(line_limit).map_err(malformed)?.as_bytes())
                .map_err(malformed)?;
            if size == 0 {
                if !skip_trailers(input, line_limit, false).map_err(malformed)? {
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

/// The longest answer line value relayed to the client.
const MAX_RELAYED_VALUE: usize = 256;
/// The most bytes of answer lines relayed to the client.
const MAX_RELAYED_BYTES: usize = 4096;

/// Whether a target's answer line `name` reaches the client: the ones a client reads to retry
/// and to show its usage limits (`retry-after`, `x-should-retry`, `request-id` and every
/// `anthropic-ratelimit-*`), as the Claude Code gateway protocol asks. No other answer line is
/// relayed; the relay writes its own framing.
fn relays_answer_line(name: &str) -> bool {
    matches!(name, "retry-after" | "x-should-retry" | "request-id")
        || name.starts_with("anthropic-ratelimit-")
}

/// The head of a target's answer.
struct Head {
    status: u16,
    content_type: Option<String>,
    framing: Answer,
    /// The `retry-after` value, kept when it is at most 64 bytes of printable ASCII.
    retry_after: Option<String>,
    /// The answer lines relayed to the client besides its framing ([`relays_answer_line`]),
    /// each `name: value\r\n`.
    relayed: String,
    /// The reason phrase of the status line, as sent.
    reason: String,
    /// Every line of the final head, its name in lower case and the line as sent: what a
    /// passthrough route relays.
    lines: Vec<(String, Vec<u8>)>,
}

/// Reads the target's answer head. Every head it sends, interim `1xx` heads included, comes out
/// of one `limit`-byte budget, so a target cannot grow memory by sending heads without end.
///
/// The status line, a field name, and the value of a field the relay reads (`content-type`,
/// `content-length`, `transfer-encoding`) must be UTF-8. The value of any other field is not
/// read, so it may carry obs-text (RFC 9110 section 5.5), as a proxy in front of a target may
/// send.
fn read_answer_head(input: &mut Buffered<'_>, limit: usize) -> io::Result<Head> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let mut used = 0_usize;
    let mut next_line = |input: &mut Buffered<'_>| {
        let left = limit.saturating_sub(used);
        if left < 2 {
            return Err(invalid());
        }
        let line = input.bytes_line(left - 2)?;
        used += line.len() + 2;
        Ok(line)
    };
    let text = |bytes: &[u8]| {
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| invalid())
    };
    loop {
        let start = text(&next_line(input)?)?;
        let mut parts = start.splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        let status = parts.next().unwrap_or_default();
        let reason = parts.next().unwrap_or_default().to_owned();
        if !version.starts_with("HTTP/1.") || status.len() != 3 {
            return Err(invalid());
        }
        let status: u16 = status.parse().map_err(|_| invalid())?;
        if !(100..=599).contains(&status) {
            return Err(invalid());
        }
        let mut content_type = None;
        let mut retry_after = None;
        let mut relayed = String::new();
        let mut length = None;
        let mut chunked = false;
        let mut lines = Vec::new();
        loop {
            let line = next_line(input)?;
            if line.is_empty() {
                break;
            }
            let colon = line
                .iter()
                .position(|&byte| byte == b':')
                .ok_or_else(invalid)?;
            let name = text(&line[..colon])?.trim().to_ascii_lowercase();
            let read = || text(&line[colon + 1..]).map(|value| value.trim().to_owned());
            lines.push((name.clone(), line.clone()));
            match name.as_str() {
                "content-type" => {
                    let value = read()?;
                    if value.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
                        content_type = Some(value);
                    }
                }
                "content-length" => length = Some(read()?.parse::<u64>().map_err(|_| invalid())?),
                "transfer-encoding" => chunked = read()?.to_ascii_lowercase().ends_with("chunked"),
                // A relayed line's value that is not short printable ASCII is not relayed, and
                // does not fail the head.
                name if relays_answer_line(name) => {
                    let Some(value) = read().ok().filter(|value| {
                        value.len() <= MAX_RELAYED_VALUE
                            && value.bytes().all(|byte| (0x20..0x7f).contains(&byte))
                    }) else {
                        continue;
                    };
                    if name == "retry-after" {
                        retry_after = Some(value.clone());
                    }
                    if relayed.len() + name.len() + value.len() + 4 <= MAX_RELAYED_BYTES {
                        relayed.push_str(name);
                        relayed.push_str(": ");
                        relayed.push_str(&value);
                        relayed.push_str("\r\n");
                    }
                }
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
            retry_after,
            relayed,
            reason,
            lines,
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
                // A target's chunk-size line is read as bytes, as its trailers are: only the
                // size before any extension is read.
                *left = chunk_size(&input.bytes_line(line_limit)?)?;
                if *left == 0 {
                    // The target's trailers are bounded like a head, and discarded.
                    if !skip_trailers(input, line_limit, true)? {
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
    /// The relay's decoder ([`Relay::with_decoder`]).
    pub(crate) decoder: Option<Arc<dyn BodyDecoder>>,
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
    /// When the request head was read: the start of the queue wait and of the time to first
    /// token (story:live-dashboard). `None` leaves both unmeasured.
    pub(crate) begun: Option<Instant>,
    /// Milliseconds from `begun` until the target's connection was open.
    pub(crate) queue_wait_ms: Option<u64>,
    /// Milliseconds from `begun` until the first answer body byte was written to the client.
    pub(crate) ttft_ms: Option<u64>,
    /// The token counts the answer reported, read as it streamed.
    pub(crate) usage: Usage,
    /// The client stopped reading before the answer's last byte.
    pub(crate) client_disconnected: bool,
    /// The `retry-after` a 429 or 5xx answer of the target carried.
    pub(crate) retry_after: Option<String>,
    /// The authority of the acquired target.
    pub(crate) upstream: Option<String>,
    /// The model name the target was sent.
    pub(crate) upstream_model: Option<String>,
    /// A digest of the request body, for the repeated-body flag.
    pub(crate) body_digest: Option<[u8; 32]>,
    /// Each attempt of a passthrough call at its target, in order.
    pub(crate) attempts: Vec<crate::dashboard::Attempt>,
    /// The label of the credential a passthrough client sent (`dashboard::credential_label`).
    pub(crate) credential: Option<String>,
    /// Why the usage is unknown though the answer carried it, e.g. `undecodable(br)`.
    pub(crate) usage_note: Option<String>,
    /// The call's entry in the dashboard's in-flight registry, removed when dropped.
    pub(crate) progress: Option<Progress>,
}

impl Call {
    /// Milliseconds since `begun`.
    fn since_begun(&self) -> Option<u64> {
        self.begun
            .map(|begun| u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX))
    }

    /// The first answer byte reached the client.
    fn first_byte(&mut self) {
        if self.ttft_ms.is_none() {
            self.ttft_ms = self.since_begun();
            if let Some(progress) = &self.progress {
                progress.streaming();
            }
        }
    }
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
        .or_else(|| {
            relay
                .models
                .values()
                .filter(|model| {
                    model
                        .model_prefix
                        .as_deref()
                        .is_some_and(|prefix| analysis.model.starts_with(prefix))
                })
                .max_by_key(|model| model.model_prefix.as_deref().map_or(0, str::len))
        })
        .ok_or(RefusalCode::ModelUnknown)?;
    call.model = Some(model.alias.as_str().to_string());
    if let Some(progress) = &call.progress {
        progress.route(model.alias.as_str());
    }
    let translator = if model.wires.contains(&wire) && !model.translated_only {
        None
    } else if model.client_credentials {
        return Err(RefusalCode::WireNotServed);
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
    if wire == Wire::Messages && analysis.offers_server_tool && !model.client_credentials {
        return Err(RefusalCode::ServerToolNotServed);
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
    call.upstream = Some(target.authority().to_owned());
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
        anthropic_beta: admitted.anthropic_beta.as_deref(),
        caller: caller(stream, admitted),
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
        target: None,
        call,
        scanner: UsageScanner::default(),
    };
    let ran = prepared.run(target.as_ref(), &mut answer);
    let Some(framing) = answer.framing else {
        // The client went away before anything was written: there is no one to answer.
        if ran.is_ok() && answer.gone {
            return Ok(());
        }
        // A target that answered is recorded with its status, as on the byte relay.
        answer.call.target_status = answer.target;
        return match ran {
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
    answer.call.target_status = answer.status();
    answer.call.response_bytes = answer.written;
    answer.call.client_disconnected = answer.gone;
    answer.call.usage = answer.scanner.usage();
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
    /// The status of the target's answer head, as the translation told it.
    target: Option<u16>,
    /// The call, told when the first body byte reached the client.
    call: &'s mut Call,
    /// Reads the answer's usage from the client-wire bytes written.
    scanner: UsageScanner,
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

    /// [`client_closed`].
    fn gone(&mut self) -> bool {
        if self.gone {
            return true;
        }
        let closed = client_closed(self.stream);
        self.gone |= closed;
        closed
    }

    fn target_status(&mut self, status: u16) {
        self.target = Some(status);
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
            if size > 0 {
                self.scanner.feed(bytes);
                self.call.first_byte();
            }
        }
        there
    }
}

/// How many bytes one look at a waiting client reads and discards at most.
const CLIENT_DISCARD: usize = 65_536;

/// A read that does not wait: the end of the stream (the client closed it, or its sending side)
/// or an error is a client gone; nothing to read yet is a client still there. The request has
/// been read whole by then, so whatever the client sent after it (a pipelined next request, RFC
/// 9112 section 9.3.2) is read and discarded, at most [`CLIENT_DISCARD`] bytes a look, and an
/// end behind it is seen: the answer carries `connection: close`, so those bytes are never
/// served. A client still sending past that bound is there. A socket whose blocking mode
/// cannot be restored is taken for gone.
fn client_closed(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let mut reader = stream;
    let mut chunk = [0_u8; 4096];
    let mut discarded = 0_usize;
    let closed = loop {
        if discarded >= CLIENT_DISCARD {
            break false;
        }
        match reader.read(&mut chunk) {
            Ok(0) => break true,
            Ok(read) => discarded += read,
            Err(error) => {
                break !matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                );
            }
        }
    };
    stream.set_nonblocking(false).is_err() || closed
}

/// How often the byte relay asks whether its client is gone while it waits for the target's
/// answer head.
const CLIENT_WATCH: Duration = Duration::from_millis(100);

/// The byte relay's watch on its client while nothing has been written to it: a client that
/// closed its connection, or its sending side, shuts the target's connection down, which ends
/// the wait for the answer head at once (story:byte-relay-client-gone). The socket shut down is
/// the one the target's stream names in [`RelayStream::shutdown_handle`], a plain `TcpStream`
/// or the socket under a TLS stream; a stream that names none is waited for.
struct ClientWatch {
    stop: mpsc::Sender<()>,
    thread: thread::JoinHandle<bool>,
}

impl ClientWatch {
    /// Starts watching `client` for `target`'s connection, or `None` when that connection names
    /// no socket or a handle on the client cannot be had.
    fn start(client: &TcpStream, target: &dyn RelayStream) -> Option<Self> {
        let target = target.shutdown_handle()?;
        let client = client.try_clone().ok()?;
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("llm-gateway-client-watch".to_owned())
            .spawn(move || {
                while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(CLIENT_WATCH)
                {
                    if client_closed(&client) {
                        drop(target.shutdown(std::net::Shutdown::Both));
                        return true;
                    }
                }
                false
            })
            .ok()?;
        Some(Self { stop, thread })
    }

    /// Stops watching, before the client is written to: whether the client was gone.
    fn end(self) -> bool {
        drop(self.stop);
        self.thread.join().unwrap_or(false)
    }
}

/// Reads the target's answer head while a [`ClientWatch`] watches `client`: `None` when the
/// client was gone first.
fn watched_head(
    client: &TcpStream,
    input: &mut Buffered<'_>,
    limit: usize,
) -> Option<io::Result<Head>> {
    let watch = match &input.source {
        Source::Target(target) => ClientWatch::start(client, &**target),
        Source::Client(..) => None,
    };
    let read = read_answer_head(input, limit);
    if watch.is_some_and(ClientWatch::end) {
        return None;
    }
    Some(read)
}

/// The target's answer head, read while watching the client: `Ok(None)` when the client went
/// away first, `Err` when no answer head arrived (`call.timed_out` says whether it timed out).
fn target_head(
    stream: &TcpStream,
    input: &mut Buffered<'_>,
    limits: &Limits,
    call: &mut Call,
) -> Result<Option<Head>, ()> {
    match watched_head(stream, input, limits.line_bytes) {
        None => Ok(None),
        Some(Ok(head)) => Ok(Some(head)),
        Some(Err(error)) => {
            call.timed_out = matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            );
            Err(())
        }
    }
}

/// The caller of `admitted`: its session header as [`Caller::new`] keeps it, and the address
/// `stream` came from, unknown when the socket cannot tell.
fn caller<'a>(stream: &TcpStream, admitted: &'a Admitted) -> Caller<'a> {
    Caller::new(
        admitted.session.as_deref(),
        stream.peer_addr().ok().map(|peer| peer.ip()),
    )
}

/// The output clamp (step 7): for a `window` and a request with `max_tokens`, counts
/// `rewritten`'s input on `target`, sent `caller` as the request will be, and clamps its
/// `max_tokens` to the room left in `window`, and answers the bytes to send. A count that fails
/// decides nothing; no room left is `context-exceeded`.
fn clamp_output(
    target: &dyn RelayTarget,
    caller: Option<&StaticHeader>,
    window: Option<u64>,
    mut rewritten: body::Rewritten,
    limits: &Limits,
) -> Result<Vec<u8>, RefusalCode> {
    let (Some(window), Some(_)) = (window, rewritten.max_tokens()) else {
        return Ok(rewritten.bytes);
    };
    let shared = Arc::new(std::mem::take(&mut rewritten.bytes));
    let counted = count_input(target, caller, &shared, limits);
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
    Ok(rewritten.bytes)
}

/// How long a [`RelayModel::with_newest`] model keeps the ID it resolved to.
pub const NEWEST_TTL: Duration = Duration::from_secs(3600);
/// The longest model list a newest-model resolution reads.
const MAX_MODEL_LIST_BYTES: usize = 1_048_576;

/// What `target` is sent for one request (steps 6 and 7): the rewritten body, clamped, and
/// the client lines forwarded with it. Records the model name sent in `call`.
fn outbound(
    (analysis, body): (&body::Analysis, &[u8]),
    (model, target): (&RelayModel, &dyn RelayTarget),
    admitted: &Admitted,
    relay: &Relay,
    limits: &Limits,
    caller_header: Option<&StaticHeader>,
    call: &mut Call,
) -> Result<(Vec<u8>, Option<ClientCredentials>), RefusalCode> {
    let forwarded = admitted
        .client_credentials
        .as_ref()
        .filter(|_| model.client_credentials)
        .map(|client| client.forwarded(target.headers(), &model.anthropic_beta));
    let resolved = model
        .newest
        .as_deref()
        .filter(|_| model.client_credentials && analysis.model == model.alias.as_str())
        .and_then(|family| newest(relay, model, family, target, forwarded.as_ref()));
    // A model that authenticates the client itself is sent the body as the client wrote it but
    // for its `model`: no effort mapping and no output clamp.
    let messages = admitted.wire == Wire::Messages && !model.client_credentials;
    let (mut rewritten, sent) = rewrite(analysis, body, model, messages, resolved.as_deref());
    call.upstream_model = Some(sent);
    if let Some(cap) = model.max_tokens_cap {
        rewritten.clamp_max_tokens(cap);
    }
    // The window of the server that answers: the one the target states, else the model's.
    let window = target
        .max_model_len()
        .or_else(|| model.window())
        .filter(|_| messages && !admitted.count_tokens);
    // The output clamp (step 7): only now, on the target `acquire` handed out, so the count
    // never wakes a pod the relay would not have woken.
    let bytes = clamp_output(target, caller_header, window, rewritten, limits)?;
    Ok((bytes, forwarded))
}

/// The newest model ID containing `family` that `target` lists, remembered for
/// [`NEWEST_TTL`]; `None` when the list cannot be read or names none.
fn newest(
    relay: &Relay,
    model: &RelayModel,
    family: &str,
    target: &dyn RelayTarget,
    client: Option<&ClientCredentials>,
) -> Option<String> {
    let alias = model.alias.as_str();
    let known = relay
        .newest
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(alias)
        .filter(|(_, at)| at.elapsed() < NEWEST_TTL)
        .map(|(id, _)| id.clone());
    if known.is_some() {
        return known;
    }
    let list = model_list(target, client)?;
    let id = newest_in(&list, family)?;
    relay
        .newest
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(alias.to_owned(), (id.clone(), Instant::now()));
    Some(id)
}

/// `GET /v1/models?limit=1000` on `target`, sent the client's lines, and the body of a 2xx
/// answer no longer than [`MAX_MODEL_LIST_BYTES`].
fn model_list(target: &dyn RelayTarget, client: Option<&ClientCredentials>) -> Option<Vec<u8>> {
    let mut connection = target.connect().ok()?;
    let client = client.map(ClientCredentials::for_listing);
    let mut head = request_head(
        target,
        None,
        MODELS_PATH,
        0,
        client.as_ref().map(ClientCredentials::as_bytes),
    );
    // The same head as a request's, asked as a `GET`: shorter, so it never reallocates.
    head.0.splice(0..4, *b"GET");
    write_request(connection.as_mut(), &head, &[]).ok()?;
    drop(head);
    let mut input = Buffered {
        source: Source::Target(connection.as_mut()),
        buffer: Vec::new(),
        start: 0,
    };
    let mut answer_head = read_answer_head(&mut input, 65_536).ok()?;
    if !(200..300).contains(&answer_head.status) {
        return None;
    }
    let mut list = Vec::new();
    while let Some(piece) = next_piece(&mut input, &mut answer_head.framing, 65_536).ok()? {
        list.extend_from_slice(&piece);
        if list.len() > MAX_MODEL_LIST_BYTES {
            return None;
        }
    }
    Some(list)
}

/// The first `"id"` string value in `list` whose lower case contains `family`: the newest, in
/// a list ordered newest first as Anthropic's is.
fn newest_in(list: &[u8], family: &str) -> Option<String> {
    let text = std::str::from_utf8(list).ok()?;
    text.match_indices("\"id\"").find_map(|(at, key)| {
        let rest = text[at + key.len()..].trim_start().strip_prefix(':')?;
        let rest = rest.trim_start().strip_prefix('"')?;
        let id = &rest[..rest.find('"')?];
        let fits = !id.is_empty()
            && id.len() <= 128
            && id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'@')
            });
        (fits && id.to_ascii_lowercase().contains(family)).then(|| id.to_owned())
    })
}

/// The body `model`'s target is sent (step 6), and the model name in it: the `resolved` newest
/// ID, else the upstream name, or the name as the client sent it when a prefix matched it; on
/// the messages wire (`messages`), every effort mapped and the model's declared effort inserted
/// where the body names none.
fn rewrite(
    analysis: &body::Analysis,
    body: &[u8],
    model: &RelayModel,
    messages: bool,
    resolved: Option<&str>,
) -> (body::Rewritten, String) {
    let upstream_model = if let Some(resolved) = resolved {
        resolved
    } else if analysis.model == model.alias.as_str() || model.model_prefix.is_none() {
        model.upstream_model.as_str()
    } else {
        analysis.model.as_str()
    };
    let rewritten = analysis.rewritten(
        body,
        upstream_model,
        messages,
        model.reasoning_effort.filter(|_| messages),
    );
    (rewritten, upstream_model.to_owned())
}

/// The request body, after a `100 Continue` when the client asked for one, read under the
/// call's deadline.
fn request_body(
    stream: &mut TcpStream,
    leftover: Vec<u8>,
    admitted: &Admitted,
    limits: &Limits,
) -> Result<Vec<u8>, RefusalCode> {
    let deadline = Instant::now() + limits.timeout;
    if admitted.expects_continue
        && !crate::server::write_by(stream, b"HTTP/1.1 100 Continue\r\n\r\n", deadline)
    {
        return Err(RefusalCode::RequestMalformed);
    }
    let mut input = Buffered {
        source: Source::Client(stream, deadline),
        buffer: leftover,
        start: 0,
    };
    read_body(&mut input, admitted.framing, limits.line_bytes)
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
    let body = request_body(stream, leftover, admitted, limits)?;
    call.body_digest = Some(auth::sha256(&body));
    let analysis = body::analyse(&body)?;
    let (model, translator) = served_model(relay, &analysis, admitted.wire, call)?;
    if let Some(translator) = translator {
        if admitted.count_tokens {
            return Err(RefusalCode::WireNotServed);
        }
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
    let alias = model.alias.as_str();
    let target = acquire(relay, alias, admitted.wire, call)?;
    // The per-caller header the target asks for (an OpenCode Go session), decided once, so the
    // count and the request carry the same value.
    let caller_header = target.caller_header(&caller(stream, admitted));
    let (rewritten, forwarded) = outbound(
        (&analysis, &body),
        (model, target.as_ref()),
        admitted,
        relay,
        limits,
        caller_header.as_ref(),
        call,
    )?;
    drop(body);
    let failed = || {
        relay.targets.invalidate(alias, target.authority());
        relay.metrics.count_invalidation();
        RefusalCode::UpstreamFailed
    };
    if let (true, Some(forwarded)) = (model.client_credentials, forwarded.as_ref()) {
        let relayed = passthrough::relay(
            stream,
            admitted,
            (model, target.as_ref()),
            (&rewritten, forwarded),
            limits,
            call,
            &failed,
        );
        // The target is released only now, after the last byte: row L12's hold.
        drop(target);
        return relayed;
    }
    let mut connection = target.connect().map_err(|_| failed())?;
    call.queue_wait_ms = call.since_begun();
    let head = request_head(
        target.as_ref(),
        caller_header.as_ref(),
        if admitted.count_tokens {
            COUNT_TOKENS_PATH
        } else {
            admitted.wire.path()
        },
        rewritten.len(),
        forwarded.as_ref().map(ClientCredentials::as_bytes),
    );
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
    // A client gone before anything was written to it: there is no one to answer, and the
    // target, whose connection the watch shut down, is released untouched.
    let Some(mut head) = target_head(stream, &mut input, limits, call).map_err(|()| failed())?
    else {
        return Ok(());
    };
    call.target_status = Some(head.status);
    if head.status == 429 || head.status >= 500 {
        call.retry_after = head.retry_after.take();
    }
    // A gateway status from the target means the pod behind it is gone or unreachable; a
    // model error arrives as 4xx or 500 instead (llmgw `src/lib.rs:629-643`).
    if matches!(head.status, 502..=504) {
        return Err(failed());
    }
    stream_answer(
        stream,
        &mut input,
        &mut head,
        limits,
        admitted.chunked_answer,
        call,
    );
    if admitted.count_tokens {
        // A count's `input_tokens` is no call's usage.
        call.usage = Usage::default();
    }
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
/// header of the client's request is forwarded, only the `caller` header its target answered
/// for it and, for a model marked [`RelayModel::with_client_credentials`] behind a gateway that
/// authenticates nobody, the `client` lines (every header but [`UNFORWARDED_HEADERS`]), which then stand in
/// for the target's own credential and for any static header of their names; so the owner's
/// credential never reaches a target. The target's bearer is one printable token (`TargetBearer::new`) and its credential
/// header one token naming no header written here (`CredentialHeader::new`), so neither can end
/// the line it is written into; nor can a static header (`StaticHeader::new`), written after
/// `host` in the order the target gives. The `caller` header, a `StaticHeader` too, is written
/// where the static header of its name stands, or after the static headers when none does, so
/// the target is sent one header of that name.
fn request_head(
    target: &dyn RelayTarget,
    caller: Option<&StaticHeader>,
    wire_path: &str,
    length: usize,
    client: Option<&[u8]>,
) -> RequestHead {
    let path = target
        .base_path()
        .map_or_else(|| wire_path.to_owned(), |base| base.join(wire_path));
    let mut start = format!("POST {path} HTTP/1.1\r\nhost: {}\r\n", target.authority());
    // A static header is never named like the target's credential header, written below when
    // the target has a key: the credential wins, and a static header cannot stand in for it.
    // Nor is the caller's header.
    let credential_name = target.credential_header().map(CredentialHeader::as_str);
    let mut caller = caller.filter(|header| credential_name != Some(header.name()));
    let mut write = |header: &StaticHeader| {
        start.push_str(header.name());
        start.push_str(": ");
        start.push_str(header.value());
        start.push_str("\r\n");
    };
    for header in target.headers() {
        if credential_name == Some(header.name())
            || (client.is_some() && matches!(header.name(), "authorization" | "x-api-key"))
        {
            continue;
        }
        match caller {
            Some(replacing) if replacing.name() == header.name() => {
                write(replacing);
                caller = None;
            }
            _ => write(header),
        }
    }
    if let Some(header) = caller {
        write(header);
    }
    let end = format!(
        "content-type: application/json\r\ncontent-length: {length}\r\nconnection: close\r\n\r\n"
    );
    // Forwarded client lines replace the target's own credential: a target is sent one.
    let bearer = target.bearer().filter(|_| client.is_none());
    let credential: [&[u8]; 4] = match (bearer, target.credential_header()) {
        (Some(bearer), Some(header)) => [header.as_str().as_bytes(), b": ", &bearer.0, b"\r\n"],
        (Some(bearer), None) => [b"authorization: Bearer ", b"", &bearer.0, b"\r\n"],
        (None, _) => [client.unwrap_or_default(), b"", b"", b""],
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
fn count_input(
    target: &dyn RelayTarget,
    caller: Option<&StaticHeader>,
    body: &Arc<Vec<u8>>,
    limits: &Limits,
) -> Option<u64> {
    let mut connection = target.connect().ok()?;
    let opened = Instant::now();
    let head = request_head(target, caller, COUNT_TOKENS_PATH, body.len(), None);
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
/// Fills `call` with the decoded body bytes written to the client, the time to the first one,
/// the token counts the answer reported and whether the client went away before its end.
fn stream_answer(
    stream: &mut TcpStream,
    input: &mut Buffered<'_>,
    head: &mut Head,
    limits: &Limits,
    chunked: bool,
    call: &mut Call,
) {
    let mut scanner = UsageScanner::default();
    let written = relay_pieces(stream, input, head, limits, chunked, call, &mut scanner);
    call.response_bytes = written;
    call.usage = scanner.usage();
}

/// [`stream_answer`]'s writing: the head, then every piece, scanned for usage as it passes.
fn relay_pieces(
    stream: &mut TcpStream,
    input: &mut Buffered<'_>,
    head: &mut Head,
    limits: &Limits,
    chunked: bool,
    call: &mut Call,
    scanner: &mut UsageScanner,
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
        "HTTP/1.1 {} {phrase}\r\n{content_type}{relayed}cache-control: no-store\r\nconnection: close\r\n{framing}\r\n",
        head.status,
        relayed = head.relayed,
    );
    let mut written: u64 = 0;
    if !crate::server::write_by(stream, start.as_bytes(), Instant::now() + limits.timeout) {
        call.client_disconnected = true;
        return written;
    }
    loop {
        match next_piece(input, &mut head.framing, limits.line_bytes) {
            Ok(Some(piece)) => {
                let size = u64::try_from(piece.len()).unwrap_or(u64::MAX);
                scanner.feed(&piece);
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
                    call.client_disconnected = true;
                    return written;
                }
                if size > 0 {
                    call.first_byte();
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
                RelayError::EmptyPrefix => model("a", vec![Wire::Messages])
                    .unwrap()
                    .with_model_prefix("")
                    .unwrap_err(),
                RelayError::EmptyFamily => model("a", vec![Wire::Messages])
                    .unwrap()
                    .with_newest("")
                    .unwrap_err(),
                RelayError::BetaNotAToken => model("a", vec![Wire::Messages])
                    .unwrap()
                    .with_anthropic_beta(vec!["oauth-2025-04-20\r\nx: y".to_owned()])
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

    impl super::RelayStream for Misaligned {}

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

    impl super::RelayStream for Interrupting {}

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
            assert_eq!(
                super::chunk_size(line.as_bytes()).ok(),
                Some(size),
                "{line:?}"
            );
        }
        // An extension may carry obs-text in a quoted value (RFC 9112 section 7.1.1).
        assert_eq!(super::chunk_size(b"a;note=\"caf\xe9\"").ok(), Some(10));
        for line in [
            "", "+10", " 10", "-1", "0x10", "10 ", "1 2", "g", "10x", ";ext",
        ] {
            assert!(super::chunk_size(line.as_bytes()).is_err(), "{line:?}");
        }
        for line in [&b"\xe9"[..], b"1\xe9", b"\xe91", b"1 \xe9;ext"] {
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
