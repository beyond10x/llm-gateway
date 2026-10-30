//! Hosted endpoints (story:hosted-endpoints): models a server somebody else runs serves, declared
//! as `[hosted.<alias>]` (`spec/domains/upstream.yaml` `HostedEndpoint`, `spec/domains/deployment.yaml`
//! `HostedDeclaration`).
//!
//! A request for a hosted model goes to the endpoint's authority, on its base path joined to the
//! wire path (story:hosted-base-path; `llm_gateway::BasePath::join`). An `http`
//! endpoint is reached over plain TCP; an `https` one over the binary's one TLS client, the pod
//! connector the composition was given ([`crate::ProxyConnector`]), whose handshake and
//! certificate verification complete before the connection is handed back, so a certificate the
//! gateway does not trust costs no request byte and no credential. The endpoint is handed out at
//! once, never held, and kept when a request through it fails: nothing replaces it.
//!
//! A translated request (story:gateway-translation) reaches the endpoint through llm's HTTP
//! client instead, over the scheme [`RelayTarget::scheme`] reports, to the base path, with the
//! credential; the endpoint's static headers are not sent on it, because llm's clients take no
//! caller headers and the headers were declared for the wire they were written for.
//!
//! An `OpenCode` Go endpoint (story:relayed-session-header; llm's `is_opencode_go` of the base
//! URL a translator reaches it at) is sent one `x-opencode-session` per request, chosen in order:
//! the caller's own, then one derived from the caller's peer address ([`derived_session`]),
//! then the static one its `headers` declare, if any ([`HostedTarget::caller_header`]).

use crate::{
    config::{Deployment, Hosted, Wire},
    keys::read_token,
    refusal::{Refusal, StartupRefusal},
    relaying::PodConnector,
    tls::{HTTPS_PORT, dial, split},
    trusted::{self, Wiped},
};
use llm_gateway::{
    AuthKind, BasePath, Caller, CredentialHeader, Label, ModelStatus, RelayStream, RelayTarget,
    RelayTargets, RouteSummary, StaticHeader, TargetBearer, TargetLimits, TargetProvenance,
    TargetRefusal, TargetScheme, TargetSummary,
};
use ring::rand::{SecureRandom as _, SystemRandom};
use rustls::pki_types::ServerName;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
    io::{self, Write as _},
    net::IpAddr,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

/// How long opening a connection to a hosted endpoint may take.
pub const ENDPOINT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read or one write on a hosted endpoint's connection may wait.
pub const ENDPOINT_IO_TIMEOUT: Duration = Duration::from_secs(120);
/// The port of an `http://` authority that names none.
const HTTP_PORT: u16 = 80;
/// The longest authority a base URL may carry.
const MAX_AUTHORITY_BYTES: usize = 256;

/// How an endpoint is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// Plain TCP.
    Http,
    /// The binary's TLS client, certificate verified.
    Https,
}

/// A base URL the gateway will not reach an endpoint at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseUrlError {
    /// Not `http://` or `https://`, an authority and an optional base path
    /// ([`llm_gateway::BasePath`]): a query, a fragment, user information, or a path outside
    /// the base path rule.
    Malformed,
    /// An `http` URL whose host is not a loopback literal (`127.0.0.0/8`, `::1`) or
    /// `localhost`, for an endpoint that is sent a credential: the credential would cross the
    /// network in cleartext.
    CleartextCredential,
}

impl fmt::Display for BaseUrlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => {
                "a base URL is http:// or https://, a host[:port] and an optional base path of \
                 segments of A-Z a-z 0-9 - . _ ~ (no empty, . or .. segment), with no query, \
                 fragment or user information"
            }
            Self::CleartextCredential => {
                "a credential is sent over http only to a loopback host (127.0.0.0/8, ::1 or \
                 localhost); use https"
            }
        })
    }
}

impl std::error::Error for BaseUrlError {}

/// The most static headers one endpoint declares.
pub const MAX_STATIC_HEADERS: usize = 32;

/// A set of static headers an endpoint is not sent (story:hosted-static-headers). Each header
/// already passed [`StaticHeader::new`]; these are the rules of the set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderSetError {
    /// More than [`MAX_STATIC_HEADERS`].
    TooMany,
    /// Two names equal without regard to case.
    Repeated,
    /// A name equal to the endpoint's own credential header: a credential stays in its file.
    CredentialHeader,
}

impl fmt::Display for HeaderSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooMany => "must hold at most 32 headers",
            Self::Repeated => "must not name one header twice, whatever the case",
            Self::CredentialHeader => {
                "must not name the endpoint's credential_header: a credential stays in \
                 credential_file"
            }
        })
    }
}

impl std::error::Error for HeaderSetError {}

/// Checks a set of static headers against the endpoint's `credential_header`.
///
/// # Errors
/// [`HeaderSetError`] for the first rule the set breaks.
pub fn check_headers(
    headers: &[StaticHeader],
    credential_header: Option<&CredentialHeader>,
) -> Result<(), HeaderSetError> {
    if headers.len() > MAX_STATIC_HEADERS {
        return Err(HeaderSetError::TooMany);
    }
    for (index, header) in headers.iter().enumerate() {
        if headers[..index]
            .iter()
            .any(|earlier| earlier.name() == header.name())
        {
            return Err(HeaderSetError::Repeated);
        }
        if credential_header.is_some_and(|credential| credential.as_str() == header.name()) {
            return Err(HeaderSetError::CredentialHeader);
        }
    }
    Ok(())
}

/// What an endpoint is sent, besides the request: nothing, a bearer, or an API key in its own
/// header. The material is a [`TargetBearer`]: one printable token, redacted in `Debug` and
/// overwritten when dropped.
#[derive(Debug)]
pub enum EndpointCredential {
    /// No credential: neither `authorization` nor any other credential header.
    Anonymous,
    /// `authorization: Bearer <credential>`.
    Bearer(TargetBearer),
    /// `<header>: <credential>`, and no `authorization`.
    ApiKey {
        header: CredentialHeader,
        key: TargetBearer,
    },
}

/// One hosted endpoint: where it is and what it is sent. It has no `Display`, and its `Debug`
/// shows no credential.
#[derive(Debug)]
pub struct HostedEndpoint {
    scheme: Scheme,
    authority: String,
    base_path: Option<BasePath>,
    credential: EndpointCredential,
    headers: Vec<StaticHeader>,
    /// Whether the base URL is `OpenCode` Go, which is sent a session per caller.
    opencode_go: bool,
}

/// What a checked base URL names: how the endpoint is reached, its authority, and the base path
/// its API lives below, if any.
pub type CheckedBaseUrl<'a> = (Scheme, &'a str, Option<BasePath>);

impl HostedEndpoint {
    /// The scheme, authority and base path of `base_url` for an endpoint authenticated as
    /// `auth_kind`: `http://` or `https://`, an authority `host[:port]` of at most 256 printable
    /// ASCII bytes, then an optional path. A path of nothing or `/` is no base path; any other
    /// is a [`BasePath`] (story:hosted-base-path), so no query, fragment, `%` or dot segment
    /// gets through. An `https` host must be a name or address a certificate can be verified
    /// for. An `http` host must be a loopback literal (`127.0.0.0/8`, `::1`) or `localhost`
    /// unless `auth_kind` is `Anonymous`, so no credential is sent in cleartext to a remote host.
    ///
    /// # Errors
    /// [`BaseUrlError::CleartextCredential`] for a credential over `http` to a remote host,
    /// [`BaseUrlError::Malformed`] for anything else.
    pub fn check(base_url: &str, auth_kind: AuthKind) -> Result<CheckedBaseUrl<'_>, BaseUrlError> {
        let (scheme, rest, default) = if let Some(rest) = base_url.strip_prefix("https://") {
            (Scheme::Https, rest, HTTPS_PORT)
        } else if let Some(rest) = base_url.strip_prefix("http://") {
            (Scheme::Http, rest, HTTP_PORT)
        } else {
            return Err(BaseUrlError::Malformed);
        };
        let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
        if authority.is_empty()
            || authority.len() > MAX_AUTHORITY_BYTES
            || !authority.bytes().all(|byte| byte.is_ascii_graphic())
            || authority.contains(['?', '#', '@', '\\', '%'])
        {
            return Err(BaseUrlError::Malformed);
        }
        let base_path = match path {
            "" | "/" => None,
            path => Some(BasePath::new(path).map_err(|_| BaseUrlError::Malformed)?),
        };
        let (host, _port) = split(authority, default).ok_or(BaseUrlError::Malformed)?;
        if host.is_empty() {
            return Err(BaseUrlError::Malformed);
        }
        if scheme == Scheme::Https && ServerName::try_from(host).is_err() {
            return Err(BaseUrlError::Malformed);
        }
        if scheme == Scheme::Http && auth_kind != AuthKind::Anonymous && !is_loopback(host) {
            return Err(BaseUrlError::CleartextCredential);
        }
        Ok((scheme, authority, base_path))
    }

    /// The endpoint at `base_url`, sent `credential`.
    ///
    /// # Errors
    /// [`BaseUrlError`] as [`Self::check`] for `credential`'s kind.
    pub fn new(base_url: &str, credential: EndpointCredential) -> Result<Self, BaseUrlError> {
        let auth_kind = match credential {
            EndpointCredential::Anonymous => AuthKind::Anonymous,
            EndpointCredential::Bearer(_) => AuthKind::Bearer,
            EndpointCredential::ApiKey { .. } => AuthKind::ApiKey,
        };
        let (scheme, authority, base_path) = Self::check(base_url, auth_kind)?;
        let opencode_go = is_opencode_go(scheme, authority, base_path.as_ref());
        Ok(Self {
            scheme,
            authority: authority.to_owned(),
            base_path,
            credential,
            headers: Vec::new(),
            opencode_go,
        })
    }

    /// The endpoint, sent `headers` after `host` on every request, in the order given.
    ///
    /// # Errors
    /// [`HeaderSetError`] as [`check_headers`] for this endpoint's credential header.
    pub fn with_headers(mut self, headers: Vec<StaticHeader>) -> Result<Self, HeaderSetError> {
        let credential_header = match &self.credential {
            EndpointCredential::ApiKey { header, .. } => Some(header),
            _ => None,
        };
        check_headers(&headers, credential_header)?;
        self.headers = headers;
        Ok(self)
    }

    /// The `host[:port]` requests are sent to and named by.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The path the endpoint's API lives below, if its base URL names one.
    pub fn base_path(&self) -> Option<&BasePath> {
        self.base_path.as_ref()
    }

    pub fn scheme(&self) -> Scheme {
        self.scheme
    }

    /// `credential set`'s probe (spec/domains/deployment.yaml `CredentialSetCommand`): one
    /// `GET <base path>/models` (`/v1/models` without a base path, as a request for it is relayed)
    /// to the authority, sent `host`, the static headers, the credential as a request to the
    /// endpoint carries it, `accept: application/json` and `connection: close`, over `tls` for
    /// `https` and plain TCP for `http`. The whole probe ends within [`PROBE_TOTAL`]; each connect,
    /// read and write is also bounded by [`PROBE_TIMEOUT`]. Interim `1xx` answers and their
    /// headers are skipped; returns the first final status, nothing past its line is read.
    ///
    /// # Errors
    /// The connection's own error, `TimedOut` past the total, or `InvalidData` for an answer that
    /// does not start with an HTTP/1 status line.
    pub(crate) fn probe(&self, tls: &dyn PodConnector) -> io::Result<u16> {
        self.probe_within(tls, PROBE_TOTAL)
    }

    /// [`Self::probe`] with `total` as its overall bound.
    pub(crate) fn probe_within(&self, tls: &dyn PodConnector, total: Duration) -> io::Result<u16> {
        let deadline = Instant::now() + total;
        let step = || -> io::Result<Duration> {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                Err(probe_timed_out())
            } else {
                Ok(left.min(PROBE_TIMEOUT))
            }
        };
        let stream: Box<dyn RelayStream> = match self.scheme {
            Scheme::Https => tls.connect_probe(&self.authority, step()?)?,
            Scheme::Http => {
                let (host, port) = split(&self.authority, HTTP_PORT).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "not a host[:port] authority")
                })?;
                let socket = dial(host, port, Instant::now() + step()?)?;
                socket.set_read_timeout(Some(step()?))?;
                socket.set_write_timeout(Some(step()?))?;
                Box::new(socket)
            }
        };
        let request = self.probe_request();
        // The exchange runs on its own thread so that the total bound holds whatever the
        // stream's own timeouts are; the thread ends by `deadline` at the latest.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || drop(sender.send(exchange(stream, request, deadline))));
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(answer) => answer,
            Err(_) => Err(probe_timed_out()),
        }
    }

    /// The probe's request, which carries the endpoint's credential: overwritten when dropped.
    fn probe_request(&self) -> Wiped {
        let path = self.base_path.as_ref().map_or_else(
            || PROBE_WIRE_PATH.to_owned(),
            |base| base.join(PROBE_WIRE_PATH),
        );
        let mut request =
            format!("GET {path} HTTP/1.1\r\nhost: {}\r\n", self.authority).into_bytes();
        for header in &self.headers {
            request
                .extend_from_slice(format!("{}: {}\r\n", header.name(), header.value()).as_bytes());
        }
        let (name, key): (&str, &[u8]) = match &self.credential {
            EndpointCredential::Anonymous => ("", &[]),
            EndpointCredential::Bearer(bearer) => ("authorization: Bearer ", bearer.expose()),
            EndpointCredential::ApiKey { header, key } => (header.as_str(), key.expose()),
        };
        if !name.is_empty() {
            request.extend_from_slice(name.as_bytes());
            if matches!(self.credential, EndpointCredential::ApiKey { .. }) {
                request.extend_from_slice(b": ");
            }
            request.extend_from_slice(key);
            request.extend_from_slice(b"\r\n");
        }
        request.extend_from_slice(b"accept: application/json\r\nconnection: close\r\n\r\n");
        Wiped::adopt(request)
    }
}

fn probe_timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "the probe took longer than its total bound",
    )
}

/// Sends `request`, overwritten once sent, and reads the first final status.
fn exchange(
    mut stream: Box<dyn RelayStream>,
    request: Wiped,
    deadline: Instant,
) -> io::Result<u16> {
    let sent = stream
        .write_all(request.as_slice())
        .and_then(|()| stream.flush());
    drop(request);
    sent?;
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "no HTTP/1 status line");
    for _ in 0..=MAX_INTERIM_ANSWERS {
        let head = read_line(&mut *stream, MAX_STATUS_LINE_BYTES, deadline)?;
        let line = std::str::from_utf8(&head).map_err(|_| malformed())?;
        let rest = line
            .strip_prefix("HTTP/1.1 ")
            .or_else(|| line.strip_prefix("HTTP/1.0 "))
            .ok_or_else(malformed)?;
        let code = rest.get(..3).ok_or_else(malformed)?;
        if !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed());
        }
        let code: u16 = code.parse().map_err(|_| malformed())?;
        if !(100..200).contains(&code) || code == 101 {
            return Ok(code);
        }
        let mut headers = 0;
        loop {
            let header = read_line(&mut *stream, MAX_STATUS_LINE_BYTES, deadline)?;
            if !header.ends_with(b"\r\n") {
                return Err(malformed());
            }
            if header == b"\r\n" {
                break;
            }
            headers += header.len();
            if headers > MAX_INTERIM_HEADER_BYTES {
                return Err(malformed());
            }
        }
    }
    Err(malformed())
}

/// One line of `stream`, a byte at a time, ended by `\r\n`, by `limit` bytes or by the end of
/// the stream; `TimedOut` once `deadline` has passed.
fn read_line(stream: &mut dyn RelayStream, limit: usize, deadline: Instant) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while !line.ends_with(b"\r\n") {
        if Instant::now() >= deadline {
            return Err(probe_timed_out());
        }
        if line.len() >= limit || stream.read(&mut byte)? == 0 {
            break;
        }
        line.push(byte[0]);
    }
    Ok(line)
}

/// How long `credential set`'s probe may take to connect, and each of its reads and writes.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the whole probe may take: connecting, the TLS handshake and the answer.
pub const PROBE_TOTAL: Duration = Duration::from_secs(30);
/// The wire path the probe asks for, which a base path replaces the leading `/v1` of.
const PROBE_WIRE_PATH: &str = "/v1/models";
/// The most interim `1xx` answers the probe skips.
const MAX_INTERIM_ANSWERS: usize = 16;
/// The most header bytes of one interim answer.
const MAX_INTERIM_HEADER_BYTES: usize = 8192;
/// The longest status line the probe reads.
const MAX_STATUS_LINE_BYTES: usize = 1024;

/// Whether `host`, as [`split`] returns it (an IPv6 literal without its brackets), names this
/// machine without a lookup: an address in `127.0.0.0/8`, `::1`, or the name `localhost`.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Whether the endpoint at `scheme`, `authority` and `base_path` is `OpenCode` Go, asked of llm
/// (`llm_messages::is_opencode_go`) for the base URL a translator reaches it at, so the byte
/// relay and llm's messages client agree on which endpoint gets a session.
fn is_opencode_go(scheme: Scheme, authority: &str, base_path: Option<&BasePath>) -> bool {
    let scheme = match scheme {
        Scheme::Http => "http",
        Scheme::Https => "https",
    };
    let base = base_path.map_or("/v1", BasePath::as_str);
    llm_providers::BaseUrl::new(&format!("{scheme}://{authority}{base}"))
        .is_ok_and(|url| llm_messages::is_opencode_go(&url))
}

/// What a derived session starts with.
const DERIVED_SESSION_PREFIX: &str = "llmgw-";
/// How many hex characters of the SHA-256 a derived session keeps: the first 16 bytes.
const DERIVED_SESSION_HEX: usize = 32;

/// The key every derived session of this process is made with: 32 bytes from the system random
/// source, drawn on first use and never written out, logged or rendered. `None` when the source
/// gave none; the gateway then derives no session.
fn session_key() -> Option<&'static [u8; 32]> {
    static KEY: OnceLock<Option<[u8; 32]>> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut key = [0_u8; 32];
        let drawn = SystemRandom::new().fill(&mut key).ok().map(|()| key);
        if drawn.is_none() {
            tracing::error!(
                "the gateway could not draw its session key; an OpenCode Go endpoint is sent no derived session"
            );
        }
        drawn
    })
    .as_ref()
}

/// The session derived from `peer` (story:relayed-session-header): `llmgw-` and the first
/// [`DERIVED_SESSION_HEX`] lowercase hex characters of SHA-256 over this process's
/// [`session_key`] and the address's octets (4 for IPv4, 16 for IPv6). One peer keeps one value
/// for the process's lifetime, two peers get two, and the address cannot be read back from it.
pub(crate) fn derived_session(peer: IpAddr) -> Option<String> {
    let key = session_key()?;
    let mut hash = Sha256::new();
    hash.update(key);
    match peer {
        IpAddr::V4(address) => hash.update(address.octets()),
        IpAddr::V6(address) => hash.update(address.octets()),
    }
    let digest = hash.finalize();
    let mut session = String::with_capacity(DERIVED_SESSION_PREFIX.len() + DERIVED_SESSION_HEX);
    session.push_str(DERIVED_SESSION_PREFIX);
    for byte in digest.iter().take(DERIVED_SESSION_HEX / 2) {
        // Writing to a String cannot fail.
        let _written = write!(session, "{byte:02x}");
    }
    Some(session)
}

/// The binary's hosted target source: each hosted model's endpoint, by alias, and `rest` for
/// every other alias (the Runpod pool, or none). An `https` endpoint is reached through `tls`.
pub struct HostedTargets {
    endpoints: BTreeMap<String, Arc<HostedEndpoint>>,
    tls: Arc<dyn PodConnector>,
    rest: Arc<dyn RelayTargets>,
}

impl HostedTargets {
    pub fn new(
        endpoints: BTreeMap<String, HostedEndpoint>,
        tls: Arc<dyn PodConnector>,
        rest: Arc<dyn RelayTargets>,
    ) -> Self {
        Self {
            endpoints: endpoints
                .into_iter()
                .map(|(alias, endpoint)| (alias, Arc::new(endpoint)))
                .collect(),
            tls,
            rest,
        }
    }
}

impl RelayTargets for HostedTargets {
    /// A hosted model's endpoint, at once; any other alias is `rest`'s.
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        match self.endpoints.get(alias) {
            Some(endpoint) => Ok(Box::new(HostedTarget {
                endpoint: Arc::clone(endpoint),
                tls: Arc::clone(&self.tls),
            })),
            None => self.rest.acquire(alias),
        }
    }

    /// A hosted endpoint that failed is kept: there is nothing to replace it with, and the next
    /// request asks it again. Any other alias is `rest`'s.
    fn invalidate(&self, alias: &str, authority: &str) {
        if !self.endpoints.contains_key(alias) {
            self.rest.invalidate(alias, authority);
        }
    }

    /// A hosted model has no lifecycle: nothing here starts its endpoint, so its wake is
    /// `None`, answered `path-unknown` (story:model-wake-and-status). Any other alias is
    /// `rest`'s.
    fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        if self.endpoints.contains_key(alias) {
            return None;
        }
        self.rest.wake(alias)
    }

    /// As [`Self::wake`]: `None` for a hosted model, `rest`'s for any other.
    fn status(&self, alias: &str) -> Option<ModelStatus> {
        if self.endpoints.contains_key(alias) {
            return None;
        }
        self.rest.status(alias)
    }
}

struct HostedTarget {
    endpoint: Arc<HostedEndpoint>,
    tls: Arc<dyn PodConnector>,
}

impl RelayTarget for HostedTarget {
    fn authority(&self) -> &str {
        &self.endpoint.authority
    }

    /// The base URL's scheme, which a translated request reaches the endpoint over.
    fn scheme(&self) -> TargetScheme {
        match self.endpoint.scheme {
            Scheme::Http => TargetScheme::Http,
            Scheme::Https => TargetScheme::Https,
        }
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        match &self.endpoint.credential {
            EndpointCredential::Anonymous => None,
            EndpointCredential::Bearer(bearer) => Some(bearer),
            EndpointCredential::ApiKey { key, .. } => Some(key),
        }
    }

    fn credential_header(&self) -> Option<&CredentialHeader> {
        match &self.endpoint.credential {
            EndpointCredential::ApiKey { header, .. } => Some(header),
            _ => None,
        }
    }

    fn base_path(&self) -> Option<&BasePath> {
        self.endpoint.base_path.as_ref()
    }

    fn headers(&self) -> &[StaticHeader] {
        &self.endpoint.headers
    }

    /// An `OpenCode` Go endpoint's `x-opencode-session` for `caller`: the caller's own, else
    /// the one derived from its peer address. `None` for any other endpoint, and for a caller
    /// with neither, which leaves the static `x-opencode-session` the endpoint declares, if any.
    fn caller_header(&self, caller: &Caller<'_>) -> Option<StaticHeader> {
        if !self.endpoint.opencode_go {
            return None;
        }
        let session = match caller.session() {
            Some(session) => session.to_owned(),
            None => derived_session(caller.peer()?)?,
        };
        StaticHeader::new(llm_messages::OPENCODE_SESSION_HEADER, &session).ok()
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        match self.endpoint.scheme {
            Scheme::Https => self.tls.connect(&self.endpoint.authority),
            Scheme::Http => {
                let (host, port) = split(&self.endpoint.authority, HTTP_PORT).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "not a host[:port] authority")
                })?;
                let socket = dial(host, port, Instant::now() + ENDPOINT_CONNECT_TIMEOUT)?;
                socket.set_read_timeout(Some(ENDPOINT_IO_TIMEOUT))?;
                socket.set_write_timeout(Some(ENDPOINT_IO_TIMEOUT))?;
                socket.set_nodelay(true)?;
                Ok(Box::new(socket))
            }
        }
    }
}

/// Every hosted model's endpoint, its credential file read once through the trusted-file reader,
/// then every route's ([`crate::Routes::read_credentials`], [`crate::Routes::hosted_endpoints`]).
///
/// # Errors
/// An `endpoint-credential:*` [`Refusal`] for the first file, in alias order, that breaks a
/// rule: every message starts `hosted.<alias>.credential_file:` and quotes neither the path nor
/// the bytes. `config:value` for a declaration the loader would not have admitted.
pub(crate) fn hosted_endpoints(
    deployment: &Deployment,
) -> Result<BTreeMap<String, HostedEndpoint>, Refusal> {
    let mut endpoints = BTreeMap::new();
    for (alias, hosted) in &deployment.hosted {
        let at = format!("hosted.{alias}");
        let invalid =
            |rule: &str| Refusal::new(StartupRefusal::ConfigValue, format!("{at} {rule}"));
        let key = format!("{at}.credential_file");
        let read = |path: &std::path::PathBuf| {
            let material = read_token(
                path,
                &key,
                &trusted::ENDPOINT_CREDENTIAL,
                (
                    StartupRefusal::EndpointCredentialTooLarge,
                    StartupRefusal::EndpointCredentialNotAToken,
                ),
            )?;
            TargetBearer::new(material).map_err(|error| {
                Refusal::new(
                    StartupRefusal::EndpointCredentialNotAToken,
                    format!("{key}: {error}"),
                )
            })
        };
        let credential = match (
            hosted.auth_kind,
            &hosted.credential_file,
            &hosted.credential_header,
        ) {
            (AuthKind::Anonymous, None, None) => EndpointCredential::Anonymous,
            (AuthKind::Bearer, Some(path), None) => EndpointCredential::Bearer(read(path)?),
            (AuthKind::ApiKey, Some(path), Some(header)) => EndpointCredential::ApiKey {
                header: header.clone(),
                key: read(path)?,
            },
            _ => return Err(invalid("declares a credential its auth_kind does not take")),
        };
        let endpoint = HostedEndpoint::new(&hosted.base_url, credential)
            .map_err(|error| invalid(&format!("base_url: {error}")))?
            .with_headers(hosted.headers.clone())
            .map_err(|error| invalid(&format!("headers {error}")))?;
        endpoints.insert(alias.clone(), endpoint);
    }
    // Each route's endpoint (story:routes-document), its `file:` secret read once per endpoint,
    // a `~/` path against `HOME`. No route name is a hosted alias (`Deployment::with_routes`).
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let credentials = deployment.routes.read_credentials(home.as_deref())?;
    endpoints.extend(deployment.routes.hosted_endpoints(&credentials)?);
    Ok(endpoints)
}

fn label(value: &str) -> Result<Label, Refusal> {
    Label::new(value).map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{value:?} cannot be served as an inventory identifier: {error}"),
        )
    })
}

/// The inventory's routes: `routes` (the Runpod models') and one per hosted model, one target
/// per wire carrying the declaration's `auth_kind` and `billing_kind`, all in alias order. No
/// target names a base URL, a credential file or a credential. Then one per route of the routes
/// document ([`crate::Routes`]).
///
/// # Errors
/// A `config:value` [`Refusal`] when a hosted model cannot be represented in an inventory.
pub(crate) fn routes(
    deployment: &Deployment,
    mut routes: Vec<RouteSummary>,
) -> Result<Vec<RouteSummary>, Refusal> {
    for (alias, hosted) in &deployment.hosted {
        let targets = targets(deployment, alias, hosted)?;
        let route =
            RouteSummary::new(label(alias)?, label(alias)?, false, targets).map_err(|error| {
                Refusal::new(StartupRefusal::ConfigValue, format!("inventory: {error}"))
            })?;
        routes.push(route);
    }
    routes.extend(crate::bedrock::summaries(deployment)?);
    routes.extend(deployment.routes.summaries()?);
    routes.sort_by(|left, right| left.alias().as_str().cmp(right.alias().as_str()));
    Ok(routes)
}

fn targets(
    deployment: &Deployment,
    alias: &str,
    hosted: &Hosted,
) -> Result<Vec<TargetSummary>, Refusal> {
    hosted
        .wires
        .iter()
        .enumerate()
        .map(|(position, wire): (usize, &Wire)| {
            Ok(TargetSummary {
                target_id: label(&format!("{alias}.{}", wire.label()))?,
                position,
                provenance: TargetProvenance {
                    protocol: label(wire.label())?,
                    provider: label("hosted")?,
                    account: label("hosted")?,
                    endpoint: label(alias)?,
                    model: label(&hosted.upstream_model)?,
                    binding_revision: label(&deployment.digest)?,
                },
                auth_kind: hosted.auth_kind,
                billing_kind: hosted.billing_kind,
                limits: TargetLimits {
                    context_window: hosted.context_window.map(u64::from),
                    max_output_tokens: None,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        BaseUrlError, EndpointCredential, HostedEndpoint, HostedTarget, Scheme, derived_session,
    };
    use crate::relaying::PodConnector;
    use crate::trusted::ledger::{self, Entry};
    use llm_gateway::AuthKind;
    use llm_gateway::BasePath;
    use llm_gateway::{
        Caller, CredentialHeader, RelayStream, RelayTarget as _, StaticHeader, TargetBearer,
    };
    use std::{
        io::{self, Read, Write},
        net::{IpAddr, TcpListener},
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    struct NoTls;

    impl PodConnector for NoTls {
        fn connect(&self, _authority: &str) -> io::Result<Box<dyn RelayStream>> {
            Err(io::Error::other("no TLS in this test"))
        }
    }

    /// A server that answers, one byte every `pause`, an interim answer that never ends: no
    /// single read waits long, so only a bound on the whole probe ends it.
    #[test]
    fn a_probe_that_is_answered_a_byte_at_a_time_ends_at_its_total_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            let mut seen = [0_u8; 1024];
            drop(connection.read(&mut seen));
            for byte in b"HTTP/1.1 103 Early Hints\r\nlink: </a>; rel=preload\r\n"
                .iter()
                .cycle()
            {
                if connection.write_all(&[*byte]).is_err() {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let endpoint = HostedEndpoint::new(
            &format!("http://{address}/v1"),
            EndpointCredential::Anonymous,
        )
        .unwrap();
        let started = Instant::now();
        let error = endpoint
            .probe_within(&NoTls, Duration::from_millis(600))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_base_url_is_a_scheme_an_authority_and_an_optional_base_path() {
        for (url, scheme, authority) in [
            ("http://127.0.0.1:8000", Scheme::Http, "127.0.0.1:8000"),
            ("http://localhost/", Scheme::Http, "localhost"),
            (
                "https://api.example.test",
                Scheme::Https,
                "api.example.test",
            ),
            (
                "https://api.example.test:8443/",
                Scheme::Https,
                "api.example.test:8443",
            ),
            ("https://[::1]:8443", Scheme::Https, "[::1]:8443"),
            ("https://127.0.0.1", Scheme::Https, "127.0.0.1"),
        ] {
            assert_eq!(
                HostedEndpoint::check(url, AuthKind::Anonymous),
                Ok((scheme, authority, None)),
                "{url}"
            );
        }
        for url in [
            "",
            "api.example.test",
            "ftp://api.example.test",
            "HTTPS://api.example.test",
            "https://",
            "https:///",
            "https://api.example.test//",
            "https://api.example.test/api/../v1",
            "https://api.example.test/v1?x=1",
            "https://api.example.test/v1#x",
            "https://user@api.example.test/v1",
            "https://api.example.test/a%2f",
            "https://api.example.test?x=1",
            "https://api.example.test#x",
            "https://user@api.example.test",
            "https://api.example.test:65536",
            "https://api.example.test:",
            "https://api example.test",
            "https://api.example.test\r\n",
            "https://-bad-.example.test",
            "https://a%2fb.test",
            "https://:443",
        ] {
            assert_eq!(
                HostedEndpoint::check(url, AuthKind::Anonymous),
                Err(BaseUrlError::Malformed),
                "{url:?}"
            );
        }
        // 257 and 256 bytes of authority, over plain http, where only the length rule decides.
        let long = format!("http://{}.test", "a.".repeat(126));
        assert_eq!(long.len() - "http://".len(), 257);
        assert!(HostedEndpoint::check(&long, AuthKind::Anonymous).is_err());
        let longest = format!("http://{}test", "a.".repeat(126));
        assert_eq!(longest.len() - "http://".len(), 256);
        assert!(HostedEndpoint::check(&longest, AuthKind::Anonymous).is_ok());
        // story:hosted-base-path: a path past the authority is the base path, a final `/`
        // dropped; the authority alone is what `host` carries.
        for (url, authority, path) in [
            (
                "https://openrouter.example.test/api/v1",
                "openrouter.example.test",
                "/api/v1",
            ),
            (
                "https://api.example.test:8443/v1/",
                "api.example.test:8443",
                "/v1",
            ),
            ("http://127.0.0.1:8000/openai", "127.0.0.1:8000", "/openai"),
        ] {
            let (_, checked, base) = HostedEndpoint::check(url, AuthKind::Bearer).unwrap();
            assert_eq!(checked, authority, "{url}");
            assert_eq!(base.as_ref().map(BasePath::as_str), Some(path), "{url}");
        }
    }

    #[test]
    fn a_credential_goes_over_http_only_to_a_loopback_host() {
        for kind in [AuthKind::Bearer, AuthKind::ApiKey] {
            for url in [
                "http://127.0.0.1:8000",
                "http://127.255.255.254",
                "http://[::1]:8000",
                "http://localhost/",
                "http://LocalHost:8000",
                "https://api.example.test",
                "https://203.0.113.10",
            ] {
                assert!(HostedEndpoint::check(url, kind).is_ok(), "{kind:?} {url}");
            }
            for url in [
                "http://203.0.113.10",
                "http://10.0.0.7:8000",
                "http://128.0.0.1",
                "http://0.0.0.0",
                "http://[::]:8000",
                "http://[::ffff:127.0.0.1]",
                "http://[2001:db8::1]",
                "http://api.example.test:8080/",
                "http://localhost.example.test",
                "http://localhost.",
            ] {
                assert_eq!(
                    HostedEndpoint::check(url, kind),
                    Err(BaseUrlError::CleartextCredential),
                    "{kind:?} {url}"
                );
                assert!(
                    HostedEndpoint::check(url, AuthKind::Anonymous).is_ok(),
                    "anonymous {url}"
                );
            }
        }
    }

    /// story:relayed-session-header: an `OpenCode` Go endpoint answers the caller's own session,
    /// else the one derived from its peer, else nothing, which leaves the static session it
    /// declares (order 3: a request with no peer address). Any other endpoint answers nothing,
    /// whatever the caller sent.
    #[test]
    fn an_opencode_go_endpoint_answers_the_callers_session_then_the_peers_then_none() {
        let target = |base_url: &str| HostedTarget {
            endpoint: Arc::new(
                HostedEndpoint::new(base_url, EndpointCredential::Anonymous)
                    .unwrap()
                    .with_headers(vec![
                        StaticHeader::new("x-opencode-session", "static-session").unwrap(),
                    ])
                    .unwrap(),
            ),
            tls: Arc::new(NoTls),
        };
        let answer = |target: &HostedTarget, session: Option<&str>, peer: Option<&str>| {
            let peer = peer.map(|peer| peer.parse::<IpAddr>().unwrap());
            target
                .caller_header(&Caller::new(session, peer))
                .map(|header| (header.name().to_owned(), header.value().to_owned()))
        };
        let go = target("https://opencode.ai/inference/go/v1");
        assert_eq!(
            answer(&go, Some("mine"), Some("10.0.0.1")),
            Some(("x-opencode-session".to_owned(), "mine".to_owned()))
        );
        let first = answer(&go, None, Some("10.0.0.1")).unwrap().1;
        assert_eq!(first, derived_session("10.0.0.1".parse().unwrap()).unwrap());
        assert_eq!(answer(&go, None, Some("10.0.0.1")).unwrap().1, first);
        assert_ne!(answer(&go, None, Some("10.0.0.2")).unwrap().1, first);
        for peer in ["10.0.0.1", "fd00::1"] {
            let value = answer(&go, None, Some(peer)).unwrap().1;
            let hex = value.strip_prefix("llmgw-").unwrap();
            assert_eq!(hex.len(), 32, "{value}");
            assert!(
                hex.bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
                "{value}"
            );
        }
        assert_eq!(answer(&go, None, None), None);
        for other in [
            "https://opencode.ai/inference/zen/v1",
            "https://opencode.ai/v1",
            "https://opencode.ai:8443/inference/go/v1",
            "http://opencode.ai/inference/go/v1",
            "https://api.example.test/inference/go/v1",
        ] {
            let target = target(other);
            assert_eq!(
                answer(&target, Some("mine"), Some("10.0.0.1")),
                None,
                "{other}"
            );
            assert_eq!(answer(&target, None, Some("10.0.0.1")), None, "{other}");
        }
    }

    /// What this proves: the probe's request, key included, is written into the one allocation
    /// taken for it before its first byte, of exactly the request's length, so no smaller one
    /// held the key and was freed by a reallocation, and that allocation reads zero in full as
    /// it is let go. That freed memory stays zero is not observable from safe Rust.
    #[test]
    fn a_probe_request_is_sized_before_its_key_is_written_and_overwritten_when_dropped() {
        const KEY: &str = "sk-probe-0123456789abcdef0123456789abcdef";
        let key = || TargetBearer::new(KEY.as_bytes().to_vec()).unwrap();
        for (credential, line) in [
            (
                EndpointCredential::Bearer(key()),
                format!("authorization: Bearer {KEY}\r\n"),
            ),
            (
                EndpointCredential::ApiKey {
                    header: CredentialHeader::new("x-api-key").unwrap(),
                    key: key(),
                },
                format!("x-api-key: {KEY}\r\n"),
            ),
            (EndpointCredential::Anonymous, String::new()),
        ] {
            let endpoint = HostedEndpoint::new("https://models.example.test/v1", credential)
                .unwrap()
                .with_headers(vec![
                    StaticHeader::new("anthropic-version", "2023-06-01").unwrap(),
                ])
                .unwrap();
            ledger::drain();
            let request = endpoint.probe_request();
            assert_eq!(
                String::from_utf8_lossy(request.as_slice()),
                format!(
                    "GET /v1/models HTTP/1.1\r\nhost: models.example.test\r\n\
                     anthropic-version: 2023-06-01\r\n{line}accept: application/json\r\n\
                     connection: close\r\n\r\n"
                )
            );
            let (address, size) = (request.as_slice().as_ptr().addr(), request.capacity());
            assert_eq!(size, request.as_slice().len(), "{line:?}");
            assert_eq!(ledger::drain(), [Entry::Taken(address, size)], "{line:?}");
            drop(request);
            assert_eq!(
                ledger::drain(),
                [Entry::Released(address, size, true)],
                "{line:?}"
            );
        }
    }
}
