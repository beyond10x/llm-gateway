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
//!
//! A hosted model's status (story:route-status-for-every-model) is one probe of its endpoint,
//! the request `credential set` sends ([`HostedEndpoint::probe_within`]), sent only when a
//! status is asked, within [`ProbeBounds::timeout`], its result reused for
//! [`ProbeBounds::reuse`] and never two at once for one model ([`HostedTargets`]).

use crate::{
    config::{Deployment, Hosted, Wire},
    keys::read_token,
    refusal::{Refusal, StartupRefusal},
    relaying::PodConnector,
    tls::{HTTPS_PORT, dial, split},
    trusted::{self, Wiped},
};
use llm_gateway::{
    AuthKind, BasePath, Caller, CredentialHeader, Label, ModelPhase, ModelStatus, RelayStream,
    RelayTarget, RelayTargets, RouteSummary, StaticHeader, TargetBearer, TargetLimits,
    TargetProvenance, TargetRefusal, TargetScheme, TargetSummary,
};
use ring::rand::{SecureRandom as _, SystemRandom};
use rustls::pki_types::ServerName;
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
    io,
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs as _},
    sync::{Arc, Mutex, OnceLock, mpsc},
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
    /// No credential of the gateway's: the client's own `authorization`, `x-api-key`,
    /// `anthropic-version` and `anthropic-beta` are forwarded instead
    /// (`llm_gateway::RelayModel::with_client_credentials`).
    Client,
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
    /// The model name an Ollama server lists, when the endpoint is an Ollama server's
    /// (story:ollama-route): its status is read from the server's lists, not from `/v1/models`.
    ollama: Option<String>,
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
            EndpointCredential::Client => AuthKind::Client,
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
            ollama: None,
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

    /// The endpoint, an Ollama server's serving `model` (story:ollama-route): its status is
    /// read from `GET /api/tags` and `GET /api/ps` ([`HostedTargets`]), and a call to a model
    /// the server answered without listing is refused `model-not-pulled`.
    #[must_use]
    pub fn with_ollama(mut self, model: &str) -> Self {
        self.ollama = Some(model.to_owned());
        self
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
        let stream = self.open_probe(tls, &system_lookup, deadline)?;
        self.exchange_probe(stream, deadline)
    }

    /// The probe's connection to the endpoint: for `http` the name looked up with `lookup` and
    /// each address tried in turn until one connects, for `https` the connector's own opening,
    /// its lookup and handshake included. Each step is bounded by what is left of `deadline` and
    /// by [`PROBE_TIMEOUT`]; the lookup itself is bounded only by its caller.
    fn open_probe(
        &self,
        tls: &dyn PodConnector,
        lookup: &dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>>,
        deadline: Instant,
    ) -> io::Result<Box<dyn RelayStream>> {
        let step = || probe_step(deadline);
        match self.scheme {
            Scheme::Https => tls.connect_probe(&self.authority, step()?),
            Scheme::Http => {
                let (host, port) = split(&self.authority, HTTP_PORT).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "not a host[:port] authority")
                })?;
                let addresses = lookup(host, port)?;
                let connect_by = Instant::now() + step()?;
                let mut last = io::Error::new(
                    io::ErrorKind::NotFound,
                    "the endpoint's host has no address",
                );
                for address in addresses {
                    let left = connect_by.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(probe_timed_out());
                    }
                    match TcpStream::connect_timeout(&address, left) {
                        Ok(socket) => {
                            socket.set_read_timeout(Some(step()?))?;
                            socket.set_write_timeout(Some(step()?))?;
                            return Ok(Box::new(socket));
                        }
                        Err(error) => last = error,
                    }
                }
                Err(last)
            }
        }
    }

    /// The probe's request over `stream` and the first final status of its answer, by
    /// `deadline`.
    fn exchange_probe(&self, stream: Box<dyn RelayStream>, deadline: Instant) -> io::Result<u16> {
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

    /// One list of an Ollama server over `stream` by `deadline` (story:ollama-route): `GET
    /// <path>` sent `host`, the static headers, `accept: application/json` and `connection:
    /// close`, and no credential. Returns the first final status and, for a 2xx, the body, read
    /// to its end within [`OLLAMA_LIST_MAX_BYTES`].
    fn exchange_list(
        &self,
        stream: Box<dyn RelayStream>,
        path: &str,
        deadline: Instant,
    ) -> io::Result<(u16, Vec<u8>)> {
        let mut start = format!("GET {path} HTTP/1.1\r\nhost: {}\r\n", self.authority);
        for header in &self.headers {
            let _written = write!(start, "{}: {}\r\n", header.name(), header.value());
        }
        start.push_str("accept: application/json\r\nconnection: close\r\n\r\n");
        let mut request = Wiped::with_capacity(start.len());
        request.extend(start.as_bytes());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || drop(sender.send(exchange_list(stream, request, deadline))));
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(answer) => answer,
            Err(_) => Err(probe_timed_out()),
        }
    }

    /// The probe's request, which carries the endpoint's credential. It is written into one
    /// allocation of exactly its length, taken before the credential is written into it, so it
    /// never reallocates and leaves no copy of the key in freed memory, and it is overwritten
    /// when dropped, as the relay's own request head is.
    fn probe_request(&self) -> Wiped {
        let path = self.base_path.as_ref().map_or_else(
            || PROBE_WIRE_PATH.to_owned(),
            |base| base.join(PROBE_WIRE_PATH),
        );
        let mut start = format!("GET {path} HTTP/1.1\r\nhost: {}\r\n", self.authority);
        for header in &self.headers {
            let _written = write!(start, "{}: {}\r\n", header.name(), header.value());
        }
        let credential: [&[u8]; 4] = match &self.credential {
            EndpointCredential::Anonymous | EndpointCredential::Client => [b""; 4],
            EndpointCredential::Bearer(bearer) => {
                [b"authorization: Bearer ", b"", bearer.expose(), b"\r\n"]
            }
            EndpointCredential::ApiKey { header, key } => {
                [header.as_str().as_bytes(), b": ", key.expose(), b"\r\n"]
            }
        };
        let end = b"accept: application/json\r\nconnection: close\r\n\r\n";
        let length =
            start.len() + credential.iter().map(|piece| piece.len()).sum::<usize>() + end.len();
        let mut request = Wiped::with_capacity(length);
        let sized = request.capacity();
        request.extend(start.as_bytes());
        for piece in credential {
            request.extend(piece);
        }
        request.extend(end);
        debug_assert_eq!(
            request.capacity(),
            sized,
            "a probe request holding a credential reallocated, leaving a copy of it in freed memory"
        );
        request
    }
}

fn probe_timed_out() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "the probe took longer than its total bound",
    )
}

/// What one step of a probe may wait: what is left of `deadline`, at most [`PROBE_TIMEOUT`];
/// `TimedOut` once the deadline has passed.
fn probe_step(deadline: Instant) -> io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(probe_timed_out())
    } else {
        Ok(left.min(PROBE_TIMEOUT))
    }
}

/// The system's name lookup, the default [`Resolver`].
fn system_lookup(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    (host, port).to_socket_addrs().map(Iterator::collect)
}

/// Sends `request`, overwritten once sent, and reads the first final status.
fn exchange(
    mut stream: Box<dyn RelayStream>,
    request: Wiped,
    deadline: Instant,
) -> io::Result<u16> {
    final_status(&mut *stream, request, deadline)
}

/// Sends an Ollama list's `request` and reads the first final status, then for a 2xx its
/// headers and its body, `content-length`, chunked or to the end of the stream, at most
/// [`OLLAMA_LIST_MAX_BYTES`] of it (story:ollama-route).
fn exchange_list(
    mut stream: Box<dyn RelayStream>,
    request: Wiped,
    deadline: Instant,
) -> io::Result<(u16, Vec<u8>)> {
    let stream = &mut *stream;
    let status = final_status(stream, request, deadline)?;
    if !(200..=299).contains(&status) {
        return Ok((status, Vec::new()));
    }
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "not an Ollama list answer");
    let (mut length, mut chunked, mut head) = (None, false, 0);
    loop {
        let line = read_line(stream, MAX_STATUS_LINE_BYTES, deadline)?;
        if !line.ends_with(b"\r\n") {
            return Err(malformed());
        }
        if line == b"\r\n" {
            break;
        }
        head += line.len();
        if head > MAX_INTERIM_HEADER_BYTES {
            return Err(malformed());
        }
        let text = std::str::from_utf8(&line).map_err(|_| malformed())?;
        if let Some((name, value)) = text.split_once(':') {
            let (name, value) = (name.trim(), value.trim());
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.parse::<usize>().map_err(|_| malformed())?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.eq_ignore_ascii_case("chunked");
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let line = read_line(stream, MAX_STATUS_LINE_BYTES, deadline)?;
            let size = std::str::from_utf8(&line)
                .ok()
                .and_then(|line| line.strip_suffix("\r\n"))
                .and_then(|line| line.split(';').next())
                .and_then(|size| usize::from_str_radix(size.trim(), 16).ok())
                .ok_or_else(malformed)?;
            if size == 0 {
                break;
            }
            if body.len().saturating_add(size) > OLLAMA_LIST_MAX_BYTES {
                return Err(malformed());
            }
            read_by(stream, &mut body, Some(size), deadline)?;
            if read_line(stream, 2, deadline)? != b"\r\n" {
                return Err(malformed());
            }
        }
    } else {
        if length.is_some_and(|length| length > OLLAMA_LIST_MAX_BYTES) {
            return Err(malformed());
        }
        read_by(stream, &mut body, length, deadline)?;
    }
    Ok((status, body))
}

/// Reads `wanted` more bytes of `stream` into `body`, or with `None` every byte to the end of the
/// stream, by `deadline` and within [`OLLAMA_LIST_MAX_BYTES`] in all.
fn read_by(
    stream: &mut dyn RelayStream,
    body: &mut Vec<u8>,
    wanted: Option<usize>,
    deadline: Instant,
) -> io::Result<()> {
    let end = wanted.map(|wanted| body.len() + wanted);
    let mut chunk = [0_u8; 8192];
    while end.is_none_or(|end| body.len() < end) {
        if Instant::now() >= deadline {
            return Err(probe_timed_out());
        }
        let room = end.map_or(chunk.len(), |end| (end - body.len()).min(chunk.len()));
        let read = stream.read(&mut chunk[..room])?;
        if read == 0 {
            return match end {
                Some(_) => Err(io::ErrorKind::UnexpectedEof.into()),
                None => Ok(()),
            };
        }
        body.extend_from_slice(&chunk[..read]);
        if body.len() > OLLAMA_LIST_MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "an Ollama list larger than its bound",
            ));
        }
    }
    Ok(())
}

/// Sends `request`, overwritten once sent, and reads the first final status, nothing past its
/// line.
fn final_status(
    stream: &mut dyn RelayStream,
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

/// The most bytes of one Ollama list's body a status read takes (story:ollama-route); a longer
/// list is no answer. Published in `docs/gateway.md` "Bounds of the status probe".
pub const OLLAMA_LIST_MAX_BYTES: usize = 1_048_576;
/// The Ollama list of the models a server has pulled.
const OLLAMA_TAGS_PATH: &str = "/api/tags";
/// The Ollama list of the models a server has loaded.
const OLLAMA_PS_PATH: &str = "/api/ps";
/// The sub-phase of an Ollama model its server does not list as pulled.
const OLLAMA_NOT_PULLED: &str = "model-not-pulled";
/// The sub-phase of an Ollama model pulled and not loaded.
const OLLAMA_NOT_LOADED: &str = "not-loaded";
/// The sub-phase of a model whose server gave no answer that could be read.
const OLLAMA_NO_ANSWER: &str = "no-answer";

/// An Ollama list, `GET /api/tags` or `GET /api/ps`: every entry's names, every other key
/// ignored.
#[derive(serde::Deserialize)]
struct OllamaList {
    models: Vec<OllamaEntry>,
}

#[derive(serde::Deserialize)]
struct OllamaEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

impl OllamaList {
    /// Whether an entry names `model`, by its `name` or its `model`: a name whose last segment
    /// (after the last `/`) carries a tag matches itself only, and one without a tag matches
    /// `<name>:latest`, as Ollama lists every model with its tag.
    fn names(&self, model: &str) -> bool {
        let tagged = model
            .rsplit('/')
            .next()
            .is_some_and(|last| last.contains(':'));
        let wanted = if tagged {
            model.to_owned()
        } else {
            format!("{model}:latest")
        };
        self.models.iter().any(|entry| {
            entry.name.as_deref() == Some(wanted.as_str())
                || entry.model.as_deref() == Some(wanted.as_str())
        })
    }
}

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

/// The bounds of a hosted model's status probe (story:route-status-for-every-model).
/// [`ProbeBounds::default`] is what the binary serves with and what `docs/gateway.md` publishes
/// ("Bounds of the status probe"); a test may shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeBounds {
    /// One probe, from opening the connection to its answer's status line; past it the model is
    /// `unreachable` `no-answer`.
    pub timeout: Duration,
    /// How long a probe's result is reused, from when the probe ended.
    pub reuse: Duration,
}

impl Default for ProbeBounds {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            reuse: Duration::from_secs(10),
        }
    }
}

/// The clock a probe's result is aged by, in milliseconds from any fixed start.
pub type ProbeClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// The name lookup of a status probe: the addresses of `host` at `port`.
pub type Resolver = Arc<dyn Fn(&str, u16) -> io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The opening of a status probe's connection, running on its own thread: what it opened before
/// the probe's deadline, or why it opened nothing, arrives here. An opening that ends at or after
/// the deadline closes what it opened itself and sends only that it has ended.
type Opening = mpsc::Receiver<io::Result<Box<dyn RelayStream>>>;

/// One hosted model's endpoint and the result of its last status probe, with when the probe
/// ended on the [`ProbeClock`]. The lock is held for the whole probe, which ends by its deadline,
/// so one model is probed by one probe at a time and a read that arrives meanwhile reads that
/// probe's result.
struct Served {
    endpoint: Arc<HostedEndpoint>,
    probed: Mutex<Option<(u64, ModelStatus)>>,
    /// An opening (the name lookup and the connection) a probe left running at its deadline.
    left_running: Mutex<Option<Opening>>,
}

/// The binary's hosted target source: each hosted model's endpoint, by alias, and `rest` for
/// every other alias (the Runpod pool, or none). An `https` endpoint is reached through `tls`.
pub struct HostedTargets {
    endpoints: BTreeMap<String, Served>,
    tls: Arc<dyn PodConnector>,
    rest: Arc<dyn RelayTargets>,
    bounds: ProbeBounds,
    clock: ProbeClock,
    resolver: Resolver,
}

impl HostedTargets {
    /// The source with the published [`ProbeBounds`], aging probe results on a monotonic clock.
    pub fn new(
        endpoints: BTreeMap<String, HostedEndpoint>,
        tls: Arc<dyn PodConnector>,
        rest: Arc<dyn RelayTargets>,
    ) -> Self {
        let start = Instant::now();
        Self {
            endpoints: endpoints
                .into_iter()
                .map(|(alias, endpoint)| {
                    let served = Served {
                        endpoint: Arc::new(endpoint),
                        probed: Mutex::new(None),
                        left_running: Mutex::new(None),
                    };
                    (alias, served)
                })
                .collect(),
            tls,
            rest,
            bounds: ProbeBounds::default(),
            clock: Arc::new(move || u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)),
            resolver: Arc::new(system_lookup),
        }
    }

    /// The same source looking endpoint names up with `resolver`. A test seam: the binary uses
    /// the system's lookup.
    #[must_use]
    pub fn with_resolver(mut self, resolver: Resolver) -> Self {
        self.resolver = resolver;
        self
    }

    /// The same source probing within `bounds`. A test seam: the binary serves the defaults.
    #[must_use]
    pub fn with_probe_bounds(mut self, bounds: ProbeBounds) -> Self {
        self.bounds = bounds;
        self
    }

    /// The same source aging probe results on `clock`. A test seam.
    #[must_use]
    pub fn with_probe_clock(mut self, clock: ProbeClock) -> Self {
        self.clock = clock;
        self
    }

    /// The status of the hosted model `served`: the last probe's while it is younger than
    /// [`ProbeBounds::reuse`], else a new probe's, sent while the model's lock is held.
    fn probed(&self, served: &Served) -> ModelStatus {
        let mut last = served
            .probed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reuse = u64::try_from(self.bounds.reuse.as_millis()).unwrap_or(u64::MAX);
        if let Some((at, status)) = last.as_ref()
            && (self.clock)().saturating_sub(*at) < reuse
        {
            return status.clone();
        }
        let client = matches!(served.endpoint.credential, EndpointCredential::Client);
        let deadline = Instant::now() + self.bounds.timeout;
        let status = if let Some(model) = &served.endpoint.ollama {
            self.ollama_status(served, model, deadline)
        } else {
            let answer = self
                .opened(served, deadline)
                .and_then(|stream| served.endpoint.exchange_probe(stream, deadline));
            probed_status(&answer, client)
        };
        *last = Some(((self.clock)(), status.clone()));
        status
    }

    /// The status of `model` on the Ollama server `served`, read by `deadline`
    /// (story:ollama-route): `GET /api/tags`, then, only when it lists the model, `GET /api/ps`.
    /// Not listed by the tags is `unreachable` `model-not-pulled`; listed by the running models
    /// `ready`, else `stopped` `not-loaded`. A list answered with a status outside 2xx is
    /// `unreachable` `status-<code>`; no answer by the deadline, or a 2xx whose body is not one
    /// list of at most [`OLLAMA_LIST_MAX_BYTES`], `unreachable` `no-answer`.
    fn ollama_status(&self, served: &Served, model: &str, deadline: Instant) -> ModelStatus {
        let listed = |path: &str| -> Result<bool, String> {
            let answer = self
                .opened(served, deadline)
                .and_then(|stream| served.endpoint.exchange_list(stream, path, deadline));
            match answer {
                Ok((200..=299, body)) => serde_json::from_slice::<OllamaList>(&body)
                    .map(|list| list.names(model))
                    .map_err(|_| OLLAMA_NO_ANSWER.to_owned()),
                Ok((code, _)) => Err(format!("status-{code:03}")),
                Err(_) => Err(OLLAMA_NO_ANSWER.to_owned()),
            }
        };
        let unreachable = |why: String| ModelStatus {
            phase: ModelPhase::Unreachable,
            sub_phase: Some(why),
            profile: None,
        };
        match listed(OLLAMA_TAGS_PATH) {
            Err(why) => unreachable(why),
            Ok(false) => unreachable(OLLAMA_NOT_PULLED.to_owned()),
            Ok(true) => match listed(OLLAMA_PS_PATH) {
                Err(why) => unreachable(why),
                Ok(true) => ModelStatus::new(ModelPhase::Ready),
                Ok(false) => ModelStatus {
                    phase: ModelPhase::Stopped,
                    sub_phase: Some(OLLAMA_NOT_LOADED.to_owned()),
                    profile: None,
                },
            },
        }
    }

    /// The probe's connection to `served`, opened by `deadline`. The opening (the name lookup
    /// with the resolver, the connection, and for `https` the connector's own lookup and
    /// handshake) runs on its own thread, so a lookup that does not return still ends the probe
    /// at its deadline. An opening still running then is left to end on its own and kept: the
    /// next probe of the model starts no second one while it runs, waits for it until its own
    /// deadline, and once it has ended opens afresh. What a late opening opened is closed by the
    /// opening itself, the moment it ends.
    fn opened(&self, served: &Served, deadline: Instant) -> io::Result<Box<dyn RelayStream>> {
        let mut left_running = served
            .left_running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(earlier) = left_running.take()
            && let Err(mpsc::RecvTimeoutError::Timeout) =
                earlier.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            *left_running = Some(earlier);
            return Err(probe_timed_out());
        }
        let (sender, receiver) = mpsc::channel();
        let endpoint = Arc::clone(&served.endpoint);
        let tls = Arc::clone(&self.tls);
        let resolver = Arc::clone(&self.resolver);
        // The hand-over: the opening checks the deadline and sends under it, and the probe
        // that stops waiting takes what was sent under it, so a connection either reaches the
        // probe in time or is closed when its opening ends, never kept in the channel.
        let handover = Arc::new(Mutex::new(()));
        let opening_handover = Arc::clone(&handover);
        std::thread::Builder::new()
            .name("llmgw-probe-open".to_owned())
            .spawn(move || {
                let opened = endpoint.open_probe(tls.as_ref(), resolver.as_ref(), deadline);
                let _held = opening_handover
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if Instant::now() >= deadline {
                    drop(opened);
                    drop(sender.send(Err(probe_timed_out())));
                } else {
                    drop(sender.send(opened));
                }
            })?;
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(opened) => opened,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let _held = handover
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Sent in the instant the wait ended: too late, and closed here.
                if let Err(mpsc::TryRecvError::Empty) = receiver.try_recv() {
                    *left_running = Some(receiver);
                }
                Err(probe_timed_out())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(
                "the probe's opening ended without an answer",
            )),
        }
    }
}

/// What a probe's answer says of its model: a 2xx status is `ready`; 401 or 403
/// `credential-refused`, but `ready` for a `client` endpoint, which holds no credential that
/// could be refused; any other status `status-<code>`; and no status line at all `no-answer`.
fn probed_status(answer: &io::Result<u16>, client: bool) -> ModelStatus {
    let unreachable = |why: String| ModelStatus {
        phase: ModelPhase::Unreachable,
        sub_phase: Some(why),
        profile: None,
    };
    match answer {
        Ok(200..=299) => ModelStatus::new(ModelPhase::Ready),
        Ok(401 | 403) if client => ModelStatus::new(ModelPhase::Ready),
        Ok(401 | 403) => unreachable("credential-refused".to_owned()),
        Ok(code) => unreachable(format!("status-{code:03}")),
        Err(_) => unreachable("no-answer".to_owned()),
    }
}

impl RelayTargets for HostedTargets {
    /// A hosted model's endpoint, at once; any other alias is `rest`'s. An Ollama model is first
    /// read as its status is, reused within [`ProbeBounds::reuse`], and refused
    /// [`TargetRefusal::NotPulled`] when its server answered its list without it
    /// (story:ollama-route); every other status hands it out, so a model not loaded is loaded by
    /// the call and a server that did not answer is asked by it.
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        match self.endpoints.get(alias) {
            Some(served)
                if served.endpoint.ollama.is_some()
                    && self.probed(served).sub_phase.as_deref() == Some(OLLAMA_NOT_PULLED) =>
            {
                Err(TargetRefusal::NotPulled)
            }
            Some(served) => Ok(Box::new(HostedTarget {
                endpoint: Arc::clone(&served.endpoint),
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

    /// A hosted model has no wake: nothing here starts its endpoint, so its wake is `None`,
    /// answered `path-unknown` (story:model-wake-and-status). Any other alias is `rest`'s.
    fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        if self.endpoints.contains_key(alias) {
            return None;
        }
        self.rest.wake(alias)
    }

    /// A hosted model's status is its endpoint's probe (story:route-status-for-every-model):
    /// within the [`ProbeBounds::timeout`], name lookup included, the request `credential set`
    /// sends ([`HostedEndpoint::probe_within`]), carrying the endpoint's own credential and static headers and nothing of any
    /// caller's, its result reused for [`ProbeBounds::reuse`]. It asks `rest` nothing. Any other
    /// alias is `rest`'s.
    fn status(&self, alias: &str) -> Option<ModelStatus> {
        match self.endpoints.get(alias) {
            Some(served) => Some(self.probed(served)),
            None => self.rest.status(alias),
        }
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
            EndpointCredential::Anonymous | EndpointCredential::Client => None,
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

/// Every hosted model's endpoint, its credential file read once through the trusted-file reader
/// or its `credential` keychain entry once ([`crate::keychain`]), then every route's
/// ([`crate::Routes::read_credentials`], [`crate::Routes::hosted_endpoints`]).
///
/// # Errors
/// An `endpoint-credential:*` [`Refusal`] for the first file, in alias order, that breaks a
/// rule: every message starts `hosted.<alias>.credential_file:` or `hosted.<alias>.credential:`
/// and quotes neither the path, the reference nor the bytes. `config:value` for a declaration
/// the loader would not have admitted.
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
        // The keychain entry `credential` names, in place of the file (story:keychain-references).
        let keychain = |reference: &crate::keychain::KeychainRef| {
            let key = format!("{at}.credential");
            let material = crate::keychain::read_keychain_token(
                reference,
                &key,
                (
                    StartupRefusal::EndpointCredentialUnreadable,
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
        let secret = match (&hosted.credential_file, &hosted.credential_keychain) {
            (None, None) => None,
            (Some(path), None) => Some(read(path)?),
            (None, Some(reference)) => Some(keychain(reference)?),
            (Some(_), Some(_)) => return Err(invalid("names its credential twice")),
        };
        let credential = match (hosted.auth_kind, secret, &hosted.credential_header) {
            (AuthKind::Anonymous, None, None) => EndpointCredential::Anonymous,
            (AuthKind::Client, None, None) => EndpointCredential::Client,
            (AuthKind::Bearer, Some(bearer), None) => EndpointCredential::Bearer(bearer),
            (AuthKind::ApiKey, Some(key), Some(header)) => EndpointCredential::ApiKey {
                header: header.clone(),
                key,
            },
            _ => return Err(invalid("declares a credential its auth_kind does not take")),
        };
        let endpoint = HostedEndpoint::new(&hosted.base_url, credential)
            .map_err(|error| invalid(&format!("base_url: {error}")))?
            .with_headers(hosted.headers.clone())
            .map_err(|error| invalid(&format!("headers {error}")))?;
        // An Ollama model's status is read from its server's lists (story:ollama-route).
        let endpoint = match hosted.kind {
            crate::config::HostedKind::Endpoint => endpoint,
            crate::config::HostedKind::Ollama => endpoint.with_ollama(&hosted.upstream_model),
        };
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
    // An Ollama model is listed under provider `ollama`, account `local` (story:ollama-route).
    let (provider, account) = match hosted.kind {
        crate::config::HostedKind::Endpoint => ("hosted", "hosted"),
        crate::config::HostedKind::Ollama => ("ollama", "local"),
    };
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
                    provider: label(provider)?,
                    account: label(account)?,
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
    use llm_gateway::{Caller, CredentialHeader, RelayStream, StaticHeader, TargetBearer};
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

    // --- The status probe (story:route-status-for-every-model) -----------------------------

    use super::{HostedTargets, ProbeBounds};
    use llm_gateway::{ModelPhase, ModelStatus, RelayTarget, RelayTargets, TargetRefusal};
    use std::{
        collections::BTreeMap,
        net::SocketAddr,
        sync::{
            Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        thread::JoinHandle,
    };

    /// The rest of the composition: records every ask and answers none.
    #[derive(Default)]
    struct Rest(Mutex<Vec<String>>);

    impl RelayTargets for Rest {
        fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
            self.0.lock().unwrap().push(format!("acquire {alias}"));
            Err(TargetRefusal::Unavailable)
        }

        fn invalidate(&self, alias: &str, _authority: &str) {
            self.0.lock().unwrap().push(format!("invalidate {alias}"));
        }

        fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
            self.0.lock().unwrap().push(format!("wake {alias}"));
            None
        }

        fn status(&self, alias: &str) -> Option<ModelStatus> {
            self.0.lock().unwrap().push(format!("status {alias}"));
            Some(ModelStatus::new(ModelPhase::Stopped))
        }
    }

    /// A loopback endpoint that reads one request head, waits `delay`, then writes `answer` (or
    /// closes without one) and hands back the head it read.
    fn endpoint_once(
        answer: Option<&'static str>,
        delay: Duration,
    ) -> (SocketAddr, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0_u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                match connection.read(&mut byte) {
                    Ok(1) => head.push(byte[0]),
                    _ => break,
                }
            }
            thread::sleep(delay);
            if let Some(answer) = answer {
                drop(connection.write_all(answer.as_bytes()));
            }
            String::from_utf8_lossy(&head).into_owned()
        });
        (address, thread)
    }

    fn targets(alias: &str, endpoint: HostedEndpoint, bounds: ProbeBounds) -> HostedTargets {
        HostedTargets::new(
            BTreeMap::from([(alias.to_owned(), endpoint)]),
            Arc::new(NoTls),
            Arc::new(Rest::default()),
        )
        .with_probe_bounds(bounds)
    }

    /// Bounds that reuse nothing, so every read probes.
    fn fresh(timeout: Duration) -> ProbeBounds {
        ProbeBounds {
            timeout,
            reuse: Duration::ZERO,
        }
    }

    fn unreachable(why: &str) -> ModelStatus {
        ModelStatus {
            phase: ModelPhase::Unreachable,
            sub_phase: Some(why.to_owned()),
            profile: None,
        }
    }

    fn bearer() -> EndpointCredential {
        EndpointCredential::Bearer(TargetBearer::new(b"sk-status-0123456789".to_vec()).unwrap())
    }

    /// Acceptance 1 and 2: the probe's status decides the phase, and only a `client` endpoint
    /// reads 401 and 403 as an endpoint that answered.
    #[test]
    fn route_status_a_hosted_status_is_ready_on_2xx_and_unreachable_saying_why_otherwise() {
        let ready = ModelStatus::new(ModelPhase::Ready);
        let cases: [(bool, Option<&'static str>, ModelStatus); 15] = [
            (
                false,
                Some("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"),
                ready.clone(),
            ),
            (
                false,
                Some("HTTP/1.1 204 No Content\r\n\r\n"),
                ready.clone(),
            ),
            (false, Some("HTTP/1.0 299 Odd\r\n\r\n"), ready.clone()),
            (
                false,
                Some("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\n\r\n"),
                ready.clone(),
            ),
            (
                false,
                Some("HTTP/1.1 301 Moved\r\n\r\n"),
                unreachable("status-301"),
            ),
            (
                false,
                Some("HTTP/1.1 401 Unauthorized\r\n\r\n"),
                unreachable("credential-refused"),
            ),
            (
                false,
                Some("HTTP/1.1 403 Forbidden\r\n\r\n"),
                unreachable("credential-refused"),
            ),
            (
                false,
                Some("HTTP/1.1 404 Not Found\r\n\r\n"),
                unreachable("status-404"),
            ),
            (
                false,
                Some("HTTP/1.1 503 Busy\r\n\r\n"),
                unreachable("status-503"),
            ),
            (
                false,
                Some("not http at all\r\n\r\n"),
                unreachable("no-answer"),
            ),
            (false, None, unreachable("no-answer")),
            (
                true,
                Some("HTTP/1.1 401 Unauthorized\r\n\r\n"),
                ready.clone(),
            ),
            (true, Some("HTTP/1.1 403 Forbidden\r\n\r\n"), ready.clone()),
            (true, Some("HTTP/1.1 200 OK\r\n\r\n"), ready.clone()),
            (
                true,
                Some("HTTP/1.1 429 Slow Down\r\n\r\n"),
                unreachable("status-429"),
            ),
        ];
        for (client, answer, expected) in cases {
            let (address, server) = endpoint_once(answer, Duration::ZERO);
            let credential = if client {
                EndpointCredential::Client
            } else {
                bearer()
            };
            let endpoint =
                HostedEndpoint::new(&format!("http://{address}/v1"), credential).unwrap();
            let targets = targets("m", endpoint, fresh(Duration::from_secs(5)));
            assert_eq!(targets.status("m"), Some(expected), "{client} {answer:?}");
            drop(server.join());
        }
        // Nothing listens: no connection is no answer.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let endpoint = HostedEndpoint::new(&format!("http://{address}"), bearer()).unwrap();
        let targets = targets("m", endpoint, fresh(Duration::from_secs(5)));
        assert_eq!(targets.status("m"), Some(unreachable("no-answer")));
    }

    /// Acceptance 3: the probe is `GET <base path>/models` with the model's own static headers
    /// and credential as its `auth_kind` sends it, and nothing else; a `client` endpoint's
    /// carries no credential.
    #[test]
    fn route_status_the_probe_carries_the_models_credential_and_headers_and_nothing_else() {
        let key = || TargetBearer::new(b"sk-status-0123456789".to_vec()).unwrap();
        let header = StaticHeader::new("x-team", "blue").unwrap();
        let cases = [
            (
                "/openai/v1",
                EndpointCredential::Bearer(key()),
                "GET /openai/v1/models HTTP/1.1\r\nhost: {authority}\r\nx-team: blue\r\nauthorization: Bearer sk-status-0123456789\r\naccept: application/json\r\nconnection: close\r\n\r\n",
            ),
            (
                "",
                EndpointCredential::ApiKey {
                    header: CredentialHeader::new("x-api-key").unwrap(),
                    key: key(),
                },
                "GET /v1/models HTTP/1.1\r\nhost: {authority}\r\nx-team: blue\r\nx-api-key: sk-status-0123456789\r\naccept: application/json\r\nconnection: close\r\n\r\n",
            ),
            (
                "/v1",
                EndpointCredential::Client,
                "GET /v1/models HTTP/1.1\r\nhost: {authority}\r\nx-team: blue\r\naccept: application/json\r\nconnection: close\r\n\r\n",
            ),
            (
                "/v1",
                EndpointCredential::Anonymous,
                "GET /v1/models HTTP/1.1\r\nhost: {authority}\r\nx-team: blue\r\naccept: application/json\r\nconnection: close\r\n\r\n",
            ),
        ];
        for (base, credential, expected) in cases {
            let (address, server) = endpoint_once(Some("HTTP/1.1 200 OK\r\n\r\n"), Duration::ZERO);
            let endpoint = HostedEndpoint::new(&format!("http://{address}{base}"), credential)
                .unwrap()
                .with_headers(vec![header.clone()])
                .unwrap();
            let targets = targets("m", endpoint, fresh(Duration::from_secs(5)));
            assert_eq!(
                targets.status("m"),
                Some(ModelStatus::new(ModelPhase::Ready))
            );
            assert_eq!(
                server.join().unwrap(),
                expected.replace("{authority}", &address.to_string())
            );
        }
    }

    /// Acceptance 3: a head that arrives inside the probe's deadline is read; one that does
    /// not, or never arrives, is `no-answer` once the deadline has passed, and no later.
    #[test]
    fn route_status_a_probe_not_answered_inside_its_deadline_is_no_answer_at_the_deadline() {
        let deadline = Duration::from_millis(600);
        let ok = Some("HTTP/1.1 200 OK\r\n\r\n");
        for (delay, expected) in [
            (
                Duration::from_millis(200),
                ModelStatus::new(ModelPhase::Ready),
            ),
            (Duration::from_millis(1200), unreachable("no-answer")),
        ] {
            let (address, server) = endpoint_once(ok, delay);
            let endpoint = HostedEndpoint::new(&format!("http://{address}/v1"), bearer()).unwrap();
            let targets = targets("m", endpoint, fresh(deadline));
            let started = Instant::now();
            assert_eq!(targets.status("m"), Some(expected), "{delay:?}");
            let took = started.elapsed();
            assert!(took < deadline + Duration::from_millis(500), "{took:?}");
            if delay > deadline {
                assert!(took >= deadline, "{took:?}");
            }
            drop(server.join());
        }
    }

    /// Acceptance 3: a result is reused until `status-reuse-seconds` have passed since its probe
    /// ended, at exactly the published number, on a clock the test moves.
    #[test]
    fn route_status_a_result_is_reused_until_the_published_reuse_time_has_passed() {
        assert_eq!(ProbeBounds::default().reuse, Duration::from_secs(10));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let probes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&probes);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut seen = [0_u8; 1024];
                drop(connection.read(&mut seen));
                drop(connection.write_all(b"HTTP/1.1 200 OK\r\n\r\n"));
            }
        });
        let now = Arc::new(AtomicU64::new(1_000));
        let clock = Arc::clone(&now);
        let endpoint = HostedEndpoint::new(&format!("http://{address}/v1"), bearer()).unwrap();
        let targets = targets("m", endpoint, ProbeBounds::default())
            .with_probe_clock(Arc::new(move || clock.load(Ordering::SeqCst)));
        let ready = Some(ModelStatus::new(ModelPhase::Ready));
        assert_eq!(targets.status("m"), ready);
        assert_eq!(probes.load(Ordering::SeqCst), 1);
        now.store(1_000 + 9_999, Ordering::SeqCst);
        assert_eq!(targets.status("m"), ready);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            1,
            "probed inside the reuse time"
        );
        now.store(1_000 + 10_000, Ordering::SeqCst);
        assert_eq!(targets.status("m"), ready);
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "not probed once the reuse time passed"
        );
    }

    /// The name lookup is inside the probe's deadline. A lookup that has not returned by then is
    /// `no-answer` at the deadline and is left running on its own: the next read starts none of
    /// its own while it runs and answers at its own deadline, and once it has ended the next read
    /// looks the name up afresh and probes.
    #[test]
    fn route_status_a_lookup_past_the_deadline_is_no_answer_and_only_one_is_left_running() {
        let timeout = Duration::from_millis(300);
        let (answering, endpoint_thread) =
            endpoint_once(Some("HTTP/1.1 200 OK\r\n\r\n"), Duration::ZERO);
        let calls = Arc::new(AtomicUsize::new(0));
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = Mutex::new(released);
        let counted = Arc::clone(&calls);
        let resolver: super::Resolver = Arc::new(move |host: &str, port: u16| {
            assert_eq!((host, port), ("localhost", 8080));
            if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                // The first lookup does not return until the test lets it.
                let _released = released.lock().unwrap().recv();
            }
            Ok(vec![answering])
        });
        let endpoint =
            HostedEndpoint::new("http://localhost:8080/v1", EndpointCredential::Anonymous).unwrap();
        let targets = targets("m", endpoint, fresh(timeout)).with_resolver(resolver);
        for read in 0..2 {
            let started = Instant::now();
            assert_eq!(
                targets.status("m"),
                Some(unreachable("no-answer")),
                "{read}"
            );
            let took = started.elapsed();
            assert!(took >= timeout, "read {read} took {took:?}");
            assert!(
                took < timeout + Duration::from_millis(500),
                "read {read} took {took:?}"
            );
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a second lookup started");
        release.send(()).unwrap();
        // The released lookup ends at once; its late answer is not used, the next read's is.
        assert_eq!(
            targets.status("m"),
            Some(ModelStatus::new(ModelPhase::Ready))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(
            endpoint_thread
                .join()
                .unwrap()
                .starts_with("GET /v1/models ")
        );
    }

    /// Acceptance 3: two reads of one model at once send one probe, and both read its result.
    #[test]
    fn route_status_two_reads_of_one_model_at_once_send_one_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let probes = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&probes);
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                let mut seen = [0_u8; 1024];
                drop(connection.read(&mut seen));
                thread::sleep(Duration::from_millis(300));
                drop(connection.write_all(b"HTTP/1.1 200 OK\r\n\r\n"));
            }
        });
        let endpoint = HostedEndpoint::new(&format!("http://{address}/v1"), bearer()).unwrap();
        let targets = Arc::new(targets(
            "m",
            endpoint,
            ProbeBounds {
                timeout: Duration::from_secs(5),
                reuse: Duration::from_secs(10),
            },
        ));
        let readers: Vec<_> = (0..2)
            .map(|_| {
                let targets = Arc::clone(&targets);
                thread::spawn(move || targets.status("m"))
            })
            .collect();
        for reader in readers {
            assert_eq!(
                reader.join().unwrap(),
                Some(ModelStatus::new(ModelPhase::Ready))
            );
        }
        assert_eq!(probes.load(Ordering::SeqCst), 1);
    }

    /// Acceptance 3 and 4: a hosted model's status asks the rest of the composition nothing,
    /// its wake stays `None`, and any other alias is still the rest's.
    #[test]
    fn route_status_a_hosted_status_asks_the_rest_nothing_and_its_wake_stays_none() {
        let (address, server) = endpoint_once(Some("HTTP/1.1 200 OK\r\n\r\n"), Duration::ZERO);
        let rest = Arc::new(Rest::default());
        let targets = HostedTargets::new(
            BTreeMap::from([(
                "m".to_owned(),
                HostedEndpoint::new(&format!("http://{address}/v1"), bearer()).unwrap(),
            )]),
            Arc::new(NoTls),
            Arc::clone(&rest) as Arc<dyn RelayTargets>,
        );
        assert_eq!(
            targets.status("m"),
            Some(ModelStatus::new(ModelPhase::Ready))
        );
        assert!(targets.wake("m").is_none());
        assert!(rest.0.lock().unwrap().is_empty(), "{:?}", rest.0.lock());
        assert_eq!(
            targets.status("pod"),
            Some(ModelStatus::new(ModelPhase::Stopped))
        );
        assert_eq!(*rest.0.lock().unwrap(), ["status pod"]);
        drop(server.join());
    }

    /// The bounds `docs/gateway.md` publishes ("Bounds of the status probe") are the defaults
    /// the binary serves with.
    #[test]
    fn route_status_the_published_probe_bounds_are_the_defaults() {
        let contract = include_str!("../../../docs/gateway.md");
        let section = contract
            .split_once("\n## Bounds of the status probe\n")
            .expect("docs/gateway.md has no section for the status probe's bounds")
            .1
            .split("\n## ")
            .next()
            .unwrap();
        let rows: BTreeMap<String, (String, u64)> = section
            .lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| {
                let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
                (
                    cells[0].trim_matches('`').to_owned(),
                    (
                        cells[1].trim_matches('`').to_owned(),
                        cells[2].parse().unwrap(),
                    ),
                )
            })
            .collect();
        let bounds = ProbeBounds::default();
        let expected: BTreeMap<String, (String, u64)> = [
            (
                "status-probe-timeout-ms",
                "ProbeBounds::timeout",
                u64::try_from(bounds.timeout.as_millis()).unwrap(),
            ),
            (
                "status-reuse-seconds",
                "ProbeBounds::reuse",
                bounds.reuse.as_secs(),
            ),
            (
                "ollama-list-max-bytes",
                "OLLAMA_LIST_MAX_BYTES",
                u64::try_from(super::OLLAMA_LIST_MAX_BYTES).unwrap(),
            ),
        ]
        .into_iter()
        .map(|(name, constant, value)| (name.to_owned(), (constant.to_owned(), value)))
        .collect();
        assert_eq!(rows, expected);
        assert_eq!(
            (bounds.timeout, bounds.reuse),
            (Duration::from_secs(5), Duration::from_secs(10))
        );
    }
}
