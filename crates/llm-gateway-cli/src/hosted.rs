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

use crate::{
    config::{Deployment, Hosted, Wire},
    keys::read_token,
    refusal::{Refusal, StartupRefusal},
    relaying::PodConnector,
    tls::{HTTPS_PORT, dial, split},
    trusted,
};
use llm_gateway::{
    AuthKind, BasePath, CredentialHeader, Label, RelayStream, RelayTarget, RelayTargets,
    RouteSummary, StaticHeader, TargetBearer, TargetLimits, TargetProvenance, TargetRefusal,
    TargetSummary,
};
use rustls::pki_types::ServerName;
use std::{
    collections::BTreeMap,
    fmt, io,
    net::IpAddr,
    sync::Arc,
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
        Ok(Self {
            scheme,
            authority: authority.to_owned(),
            base_path,
            credential,
            headers: Vec::new(),
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
}

/// Whether `host`, as [`split`] returns it (an IPv6 literal without its brackets), names this
/// machine without a lookup: an address in `127.0.0.0/8`, `::1`, or the name `localhost`.
fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
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
}

struct HostedTarget {
    endpoint: Arc<HostedEndpoint>,
    tls: Arc<dyn PodConnector>,
}

impl RelayTarget for HostedTarget {
    fn authority(&self) -> &str {
        &self.endpoint.authority
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

/// Every hosted model's endpoint, its credential file read once through the trusted-file reader.
///
/// # Errors
/// An `endpoint-credential:*` [`Refusal`] for the first file, in alias order, that breaks a
/// rule: every message starts `hosted.<alias>.credential_file <path>:` and never quotes the
/// bytes. `config:value` for a declaration the loader would not have admitted.
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
/// target names a base URL, a credential file or a credential.
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
    use super::{BaseUrlError, HostedEndpoint, Scheme};
    use llm_gateway::AuthKind;
    use llm_gateway::BasePath;

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
}
