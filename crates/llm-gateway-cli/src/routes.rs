//! The models of an llm `llm.routes/1` document a deployment names by its `routes` key
//! (story:routes-document; `spec/domains/deployment.yaml` `RoutesDeclaration`).
//!
//! The document is read once at start through the trusted-file reader and parsed with llm 0.11.0's
//! `Routes::parse`, which validates it strictly and performs no I/O; `llm-models` is linked
//! without its secret-source features, so llm reads no secret. What llm's API does not expose,
//! the path of an endpoint's `file:` secret, is read here from the same text with a minimal
//! reader that ignores everything else. Each route becomes a served model whose endpoint is a
//! [`HostedEndpoint`] derived from the route's endpoint: its wire, base URL, authentication and
//! credential, and for a Messages endpoint llm's `anthropic-version` as its one static header. A
//! request on the endpoint's own wire is relayed byte for byte to its base path; a request on
//! another wire is translated under the route's capabilities.
//!
//! Credentials come only from trusted files, as every other credential of the gateway: an
//! `env:` or `keychain:` secret is refused, and a `file:` secret is read once at start with the
//! endpoint-credential policy, a `~/` path resolved against `HOME`.

use crate::{
    config::{Wire, hex_digest},
    hosted::{BaseUrlError, EndpointCredential, HostedEndpoint},
    keys::read_token,
    refusal::{Refusal, StartupRefusal},
    relaying::relay_wire,
    translate::LlmTranslator,
    trusted,
};
use llm_core::{Capabilities, Protocol};
use llm_gateway::{
    AuthKind, BillingKind, CredentialHeader, Label, RelayModel, RouteSummary, StaticHeader,
    TargetBearer, TargetLimits, TargetProvenance, TargetSummary, ToolCalling,
};
use llm_models::routes::{EndpointDeclaration, SecretKind};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The provider and account every route's target is listed under.
const PROVIDER: &str = "routes";

/// The longest label of a wire, which a route's inventory target id `<route>.<wire>` carries.
const LONGEST_WIRE_LABEL: usize = {
    let wires = [Wire::Chat, Wire::Responses, Wire::Messages];
    let mut longest = 0;
    let mut index = 0;
    while index < wires.len() {
        let length = wires[index].label().len();
        if length > longest {
            longest = length;
        }
        index += 1;
    }
    longest
};

/// The longest inventory label, `llm_gateway::Label`'s bound
/// (`tests::the_label_bound_is_the_gateways`).
const LABEL_BYTES: usize = 256;

/// The longest route name the gateway serves: the longest whose target id `<route>.<wire>` fits
/// one inventory label on every wire. llm admits up to 256 bytes; the gateway, fewer.
pub const MAX_ROUTE_NAME_BYTES: usize = LABEL_BYTES - 1 - LONGEST_WIRE_LABEL;

/// How much of a refused route name a message quotes.
const QUOTED_NAME_BYTES: usize = 32;

/// Where an endpoint's `file:` secret lives: an absolute path, or one below `HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretFile {
    Absolute(PathBuf),
    /// Relative to the home directory (`file:~/<path>`).
    Home(PathBuf),
}

impl SecretFile {
    /// The file's path, a [`Self::Home`] one joined to `home`; `None` without an absolute home.
    pub fn resolve(&self, home: Option<&Path>) -> Option<PathBuf> {
        match self {
            Self::Absolute(path) => Some(path.clone()),
            Self::Home(relative) => home
                .filter(|home| home.is_absolute())
                .map(|home| home.join(relative)),
        }
    }
}

/// One endpoint of the document, checked against the gateway's rules.
#[derive(Debug, Clone)]
pub struct RouteEndpoint {
    name: String,
    /// The base URL as llm normalises it; never rendered by the inventory.
    base_url: String,
    wire: Wire,
    auth_kind: AuthKind,
    credential_header: Option<CredentialHeader>,
    secret: Option<SecretFile>,
    billing_kind: BillingKind,
}

impl RouteEndpoint {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The wire the endpoint speaks: its `protocol`.
    pub fn wire(&self) -> Wire {
        self.wire
    }

    pub fn auth_kind(&self) -> AuthKind {
        self.auth_kind
    }

    /// llm's expansion: `self-hosted` for an anonymous endpoint, `metered` otherwise.
    pub fn billing_kind(&self) -> BillingKind {
        self.billing_kind
    }

    /// The `file:` secret's location; `None` for an anonymous endpoint.
    pub fn secret(&self) -> Option<&SecretFile> {
        self.secret.as_ref()
    }

    /// The base URL as llm normalises it, for `credential set`'s probe; never rendered.
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The header an `api-key` endpoint receives its credential in.
    pub(crate) fn credential_header(&self) -> Option<&CredentialHeader> {
        self.credential_header.as_ref()
    }

    /// The static headers every request to the endpoint is sent ([`endpoint_headers`]).
    pub(crate) fn static_headers(&self) -> Result<Vec<StaticHeader>, Refusal> {
        endpoint_headers(self.wire)
    }
}

/// One route of the document: a served model.
#[derive(Debug, Clone)]
pub struct Route {
    name: String,
    endpoint: String,
    upstream_model: String,
    capabilities: Capabilities,
}

impl Route {
    /// The route's name: the model alias it is served under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The name of the endpoint that serves it.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The model id sent upstream.
    pub fn upstream_model(&self) -> &str {
        &self.upstream_model
    }

    /// llm's declared capabilities, or its defaults for a route written as a plain string.
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}

/// A checked routes document: its endpoints and routes by name, and the digest of its bytes.
/// Empty when the deployment names none.
#[derive(Debug, Clone, Default)]
pub struct Routes {
    endpoints: BTreeMap<String, RouteEndpoint>,
    declared: BTreeMap<String, Route>,
    digest: String,
}

/// The only part of the document llm's API does not expose: each endpoint's `auth.secret`.
/// Every other field is ignored; llm has already refused a document breaking its rules.
#[derive(Deserialize)]
struct SecretsDocument {
    #[serde(default)]
    endpoints: BTreeMap<String, SecretsEndpoint>,
}

#[derive(Deserialize)]
struct SecretsEndpoint {
    auth: SecretsAuth,
}

#[derive(Deserialize)]
struct SecretsAuth {
    #[serde(default)]
    secret: Option<String>,
}

fn value(key: &str, rule: &str) -> Refusal {
    Refusal::new(StartupRefusal::ConfigValue, format!("{key} {rule}"))
}

fn label(key: &str, text: &str) -> Result<Label, Refusal> {
    Label::new(text).map_err(|error| value(key, &format!("cannot be served: {error}")))
}

impl Routes {
    /// Parses and checks a routes document's text. Reads no file.
    ///
    /// # Errors
    /// `config:value`: `routes <llm's message>` for a document llm refuses, and otherwise naming
    /// `routes.<endpoint>.auth.secret` for an `env:` or `keychain:` secret,
    /// `routes.<endpoint>.base_url` for a URL without its API path, one outside the base-path
    /// rule or one sending a credential over `http` to a remote host, and
    /// `routes.<endpoint>.auth.header` for a header the gateway will not send a credential in.
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        let parsed = llm_models::routes::Routes::parse(text)
            .map_err(|error| value("routes", &error.message))?;
        let secrets: SecretsDocument = serde_yaml::from_str(text)
            .map_err(|_| value("routes", "cannot be read for its secret references"))?;
        let mut endpoints = BTreeMap::new();
        for declaration in parsed.endpoints() {
            let name = declaration.name().as_str();
            let declared = secrets
                .endpoints
                .get(name)
                .and_then(|endpoint| endpoint.auth.secret.as_deref());
            endpoints.insert(name.to_owned(), endpoint(declaration, declared)?);
        }
        let mut routes = BTreeMap::new();
        for route in parsed.routes() {
            let name = route.name().as_str().to_owned();
            route_name(&name)?;
            routes.insert(
                name.clone(),
                Route {
                    name,
                    endpoint: route.endpoint().as_str().to_owned(),
                    upstream_model: route.upstream_model().as_str().to_owned(),
                    capabilities: route.capabilities().clone(),
                },
            );
        }
        Ok(Self {
            endpoints,
            declared: routes,
            digest: hex_digest(text.as_bytes()),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.declared.is_empty()
    }

    /// Every route, in name order.
    pub fn routes(&self) -> impl Iterator<Item = &Route> {
        self.declared.values()
    }

    /// Every endpoint, in name order.
    pub fn endpoints(&self) -> impl Iterator<Item = &RouteEndpoint> {
        self.endpoints.values()
    }

    /// SHA-256 of the exact document bytes, as 64 lowercase hex characters.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The endpoint `route` names.
    ///
    /// # Errors
    /// `config:value` naming `routes.<route>` for an undeclared endpoint.
    pub(crate) fn endpoint_of(&self, route: &Route) -> Result<&RouteEndpoint, Refusal> {
        self.endpoints.get(&route.endpoint).ok_or_else(|| {
            value(
                &format!("routes.{}", route.name),
                "names an undeclared endpoint",
            )
        })
    }

    /// Every route as the relay serves it: under its name, on its endpoint's wire, relayed under
    /// its model id, its tool calling `parsed` exactly when its capabilities say `tools`, and
    /// listing its capabilities' context window.
    ///
    /// # Errors
    /// `config:value` for a route that cannot be relayed.
    pub fn relay_models(&self) -> Result<Vec<RelayModel>, Refusal> {
        self.routes()
            .map(|route| {
                let at = format!("routes.{}", route.name);
                let endpoint = self.endpoint_of(route)?;
                let model = RelayModel::new(
                    label(&at, &route.name)?,
                    label(&at, &route.upstream_model)?,
                    vec![relay_wire(endpoint.wire)],
                )
                .map_err(|error| value(&at, &error.to_string()))?;
                Ok(model
                    .with_tool_calling(if route.capabilities.tools {
                        ToolCalling::Parsed
                    } else {
                        ToolCalling::Absent
                    })
                    .with_context_window(route.capabilities.context_window))
            })
            .collect()
    }

    /// `translator`, translating each route under its declared capabilities.
    #[must_use]
    pub fn translator(&self, translator: LlmTranslator) -> LlmTranslator {
        self.routes().fold(translator, |translator, route| {
            translator.with_capabilities(&route.name, route.capabilities.clone())
        })
    }

    /// Each credentialed endpoint's credential, by endpoint name: its `file:` secret read once
    /// through the trusted-file reader with the endpoint-credential policy, a `~/` path joined
    /// to `home`.
    ///
    /// # Errors
    /// `config:value` naming `routes.<endpoint>.auth.secret` for a `~/` path without an absolute
    /// `home`; an `endpoint-credential:*` [`Refusal`] whose message starts
    /// `routes.<endpoint>.auth.secret <path>:` for a file breaking a rule. No message quotes a
    /// credential.
    pub fn read_credentials(
        &self,
        home: Option<&Path>,
    ) -> Result<BTreeMap<String, TargetBearer>, Refusal> {
        let mut credentials = BTreeMap::new();
        for endpoint in self.endpoints() {
            let Some(secret) = &endpoint.secret else {
                continue;
            };
            let key = format!("routes.{}.auth.secret", endpoint.name);
            let path = secret.resolve(home).ok_or_else(|| {
                value(
                    &key,
                    "names a file:~/ path, and HOME is not set to an absolute directory",
                )
            })?;
            let material = read_token(
                &path,
                &key,
                &trusted::ENDPOINT_CREDENTIAL,
                (
                    StartupRefusal::EndpointCredentialTooLarge,
                    StartupRefusal::EndpointCredentialNotAToken,
                ),
            )?;
            let bearer = TargetBearer::new(material).map_err(|error| {
                Refusal::new(
                    StartupRefusal::EndpointCredentialNotAToken,
                    format!("{key}: {error}"),
                )
            })?;
            credentials.insert(endpoint.name.clone(), bearer);
        }
        Ok(credentials)
    }

    /// Each route's hosted endpoint, by route name, sent the credential `credentials` holds under
    /// its endpoint's name.
    ///
    /// # Errors
    /// `config:value` naming `routes.<endpoint>.auth.secret` for a credentialed endpoint
    /// `credentials` holds nothing for, and `routes.<endpoint>.base_url` for a URL the hosted
    /// endpoint refuses.
    pub fn hosted_endpoints(
        &self,
        credentials: &BTreeMap<String, TargetBearer>,
    ) -> Result<BTreeMap<String, HostedEndpoint>, Refusal> {
        let mut hosted = BTreeMap::new();
        for route in self.routes() {
            let endpoint = self.endpoint_of(route)?;
            let key = format!("routes.{}.auth.secret", endpoint.name);
            let bearer = || {
                let held = credentials
                    .get(&endpoint.name)
                    .ok_or_else(|| value(&key, "has no credential"))?;
                TargetBearer::new(held.expose().to_vec()).map_err(|error| {
                    Refusal::new(
                        StartupRefusal::EndpointCredentialNotAToken,
                        format!("{key}: {error}"),
                    )
                })
            };
            let credential = match (endpoint.auth_kind, &endpoint.credential_header) {
                (AuthKind::Anonymous, _) => EndpointCredential::Anonymous,
                (AuthKind::Bearer, _) => EndpointCredential::Bearer(bearer()?),
                (AuthKind::ApiKey, Some(header)) => EndpointCredential::ApiKey {
                    header: header.clone(),
                    key: bearer()?,
                },
                (AuthKind::ApiKey, None) => {
                    return Err(value(
                        &format!("routes.{}.auth.header", endpoint.name),
                        "is required for an api-key endpoint",
                    ));
                }
                (AuthKind::AwsSigv4, _) => {
                    return Err(value(
                        &format!("routes.{}.auth", endpoint.name),
                        "is no auth kind a routes-document endpoint takes",
                    ));
                }
            };
            let served = HostedEndpoint::new(&endpoint.base_url, credential)
                .map_err(|error| {
                    value(
                        &format!("routes.{}.base_url", endpoint.name),
                        &error.to_string(),
                    )
                })?
                .with_headers(endpoint_headers(endpoint.wire)?)
                .map_err(|error| {
                    value(
                        &format!("routes.{}.auth.header", endpoint.name),
                        &format!("cannot be sent beside anthropic-version: {error}"),
                    )
                })?;
            hosted.insert(route.name.clone(), served);
        }
        Ok(hosted)
    }

    /// The inventory's routes: one per route name, its one target naming the endpoint's
    /// protocol, provider and account `routes`, the endpoint's name, the route's model id and
    /// the document's digest, its kinds and its declared limits. No target names a base URL, a
    /// credential file or a credential.
    pub(crate) fn summaries(&self) -> Result<Vec<RouteSummary>, Refusal> {
        self.routes()
            .map(|route| {
                let at = format!("routes.{}", route.name);
                let endpoint = self.endpoint_of(route)?;
                let target = TargetSummary {
                    target_id: label(&at, &format!("{}.{}", route.name, endpoint.wire.label()))?,
                    position: 0,
                    provenance: TargetProvenance {
                        protocol: label(&at, endpoint.wire.label())?,
                        provider: label(&at, PROVIDER)?,
                        account: label(&at, PROVIDER)?,
                        endpoint: label(&at, &endpoint.name)?,
                        model: label(&at, &route.upstream_model)?,
                        binding_revision: label(&at, &self.digest)?,
                    },
                    auth_kind: endpoint.auth_kind,
                    billing_kind: endpoint.billing_kind,
                    limits: TargetLimits {
                        context_window: Some(route.capabilities.context_window),
                        max_output_tokens: Some(route.capabilities.max_output_tokens),
                    },
                };
                RouteSummary::new(
                    label(&at, &route.name)?,
                    label(&at, &route.name)?,
                    false,
                    vec![target],
                )
                .map_err(|error| value("inventory:", &error.to_string()))
            })
            .collect()
    }
}

/// One endpoint, checked: its secret a `file:` reference, its base URL naming its API path
/// within the base-path rule, its credential never in cleartext to a remote host, and an
/// `api-key` endpoint's header one the gateway sends a credential in.
fn endpoint(
    declaration: &EndpointDeclaration,
    secret: Option<&str>,
) -> Result<RouteEndpoint, Refusal> {
    let name = declaration.name().as_str();
    let key = |field: &str| format!("routes.{name}.{field}");
    let auth_kind = match declaration.auth_kind() {
        llm_core::AuthKind::Anonymous => AuthKind::Anonymous,
        llm_core::AuthKind::Bearer => AuthKind::Bearer,
        llm_core::AuthKind::ApiKey => AuthKind::ApiKey,
        llm_core::AuthKind::SubscriptionOauth => {
            return Err(value(
                &key("auth.kind"),
                "must be anonymous, bearer or api-key",
            ));
        }
    };
    let secret = match declaration.secret_kind() {
        None => None,
        Some(SecretKind::Environment | SecretKind::Keychain) => {
            return Err(value(
                &key("auth.secret"),
                "must be a file: reference: the gateway reads a credential only from a trusted \
                 file, never from an env: variable or a keychain: entry",
            ));
        }
        Some(SecretKind::File) => Some(secret_file(&key("auth.secret"), secret)?),
    };
    let base_url = declaration.base_url().as_str().to_owned();
    match HostedEndpoint::check(&base_url, auth_kind) {
        Ok((_, _, Some(_))) => {}
        Ok((_, _, None)) => {
            return Err(value(
                &key("base_url"),
                "must name the endpoint's API path, such as https://api.example.com/v1: llm \
                 appends the wire path to it, and the gateway replaces the wire path's leading \
                 /v1 with it",
            ));
        }
        Err(BaseUrlError::Malformed) => {
            return Err(value(
                &key("base_url"),
                "must be http:// or https://, a host[:port] of at most 256 printable ASCII \
                 characters and a base path of at most 32 segments and 256 bytes, each segment \
                 one or more of A-Z a-z 0-9 - . _ ~ and neither . nor ..",
            ));
        }
        Err(BaseUrlError::CleartextCredential) => {
            return Err(value(
                &key("base_url"),
                "must be https:// for a credential sent to a host that is not loopback \
                 (127.0.0.0/8, ::1 or localhost): a credential is never sent in cleartext",
            ));
        }
    }
    let credential_header = declaration
        .api_key_header()
        .map(|header| {
            CredentialHeader::new(header.as_str())
                .map_err(|error| value(&key("auth.header"), &error.to_string()))
        })
        .transpose()?;
    let wire = match declaration.protocol() {
        Protocol::Messages => Wire::Messages,
        Protocol::Responses => Wire::Responses,
        Protocol::ChatCompletions => Wire::Chat,
        // llm's routes document refuses this protocol itself; a Bedrock model is a
        // `[hosted.<alias>]` entry of kind `bedrock`.
        Protocol::BedrockConverse => {
            return Err(value(
                &key("protocol"),
                "must be messages, responses or chat-completions: a Bedrock model is declared \
                 as [hosted.<alias>] with kind = \"bedrock\"",
            ));
        }
    };
    Ok(RouteEndpoint {
        name: name.to_owned(),
        base_url,
        wire,
        billing_kind: if auth_kind == AuthKind::Anonymous {
            BillingKind::SelfHosted
        } else {
            BillingKind::Metered
        },
        auth_kind,
        credential_header,
        secret,
    })
}

/// The static headers a route's endpoint is sent on every request it is relayed, the output
/// clamp's count included: a Messages endpoint gets llm's `anthropic-version`, which Anthropic's
/// API requires and llm's own client sends on the translated path; any other endpoint none.
fn endpoint_headers(wire: Wire) -> Result<Vec<StaticHeader>, Refusal> {
    if wire != Wire::Messages {
        return Ok(Vec::new());
    }
    let header = StaticHeader::new(
        llm_messages::VERSION_HEADER,
        llm_messages::ANTHROPIC_VERSION,
    )
    .map_err(|error| value("routes", &format!("anthropic-version: {error}")))?;
    Ok(vec![header])
}

/// A route name the gateway can serve: at most [`MAX_ROUTE_NAME_BYTES`], so that its target id
/// `<route>.<wire>` fits one inventory label. A longer name is quoted only in part.
fn route_name(name: &str) -> Result<(), Refusal> {
    if name.len() <= MAX_ROUTE_NAME_BYTES {
        return Ok(());
    }
    // llm admits only printable ASCII, so any byte offset is a character boundary.
    let quoted = name.get(..QUOTED_NAME_BYTES).unwrap_or(name);
    Err(value(
        &format!("routes.{quoted}…"),
        &format!(
            "is a route name of {} bytes; the gateway serves route names of at most \
             {MAX_ROUTE_NAME_BYTES} bytes, so that the target id <route>.<wire> fits its \
             {LABEL_BYTES}-byte inventory label",
            name.len()
        ),
    ))
}

/// A `file:` reference's path: `~/<path>` below the home directory, or an absolute path.
fn secret_file(key: &str, secret: Option<&str>) -> Result<SecretFile, Refusal> {
    let path = secret
        .and_then(|secret| secret.strip_prefix("file:"))
        .ok_or_else(|| value(key, "must be a file: reference"))?;
    if let Some(relative) = path.strip_prefix("~/") {
        Ok(SecretFile::Home(PathBuf::from(relative)))
    } else if path.starts_with('/') {
        Ok(SecretFile::Absolute(PathBuf::from(path)))
    } else {
        Err(value(key, "must name an absolute or ~/ path"))
    }
}

/// Reads the routes document at `path` through the trusted-file reader and checks it.
///
/// # Errors
/// A `config:*` [`Refusal`] whose message starts `routes`: a trusted-file rule, with the routes
/// document's 2 MiB bound, or [`Routes::parse`]'s.
pub(crate) fn load(path: &Path) -> Result<Routes, Refusal> {
    let text = trusted::read(path, &trusted::ROUTES)
        .map_err(|refusal| Refusal::new(refusal.kind(), format!("routes {}", refusal.message())))?;
    Routes::parse(&text)
}

#[cfg(test)]
mod tests {
    use super::{Routes, SecretFile};
    use crate::trusted;
    use std::path::{Path, PathBuf};

    const DOCUMENT: &str = "format: llm.routes/1\nendpoints:\n  lab: {base_url: http://127.0.0.1:8000/v1, protocol: chat-completions, auth: {kind: anonymous}}\nroutes:\n  small: lab/small\n";

    /// The routes document's bound is llm's own: a document of exactly the bound parses, and
    /// one byte more is refused by `Routes::parse` itself.
    #[test]
    fn the_routes_bound_is_the_bound_llm_parses_to() {
        let limit = usize::try_from(trusted::ROUTES.limit).unwrap();
        let padded = |size: usize| {
            let mut text = format!("{DOCUMENT}#");
            text.push_str(&"a".repeat(size - text.len() - 1));
            text.push('\n');
            text
        };
        assert!(Routes::parse(&padded(limit)).is_ok());
        let refused = Routes::parse(&padded(limit + 1)).unwrap_err();
        assert_eq!(refused.code(), "config:value");
        assert!(
            refused.message().starts_with("routes ") && refused.message().contains("byte bound"),
            "{refused}"
        );
    }

    /// `LABEL_BYTES` is the gateway's own label bound: a label of exactly it is admitted, one byte
    /// more is not.
    #[test]
    fn the_label_bound_is_the_gateways() {
        assert!(llm_gateway::Label::new("a".repeat(super::LABEL_BYTES)).is_ok());
        assert!(llm_gateway::Label::new("a".repeat(super::LABEL_BYTES + 1)).is_err());
        assert_eq!(super::LONGEST_WIRE_LABEL, "responses".len());
    }

    /// A route name of [`super::MAX_ROUTE_NAME_BYTES`] is served on every wire, its target id
    /// fitting one label; one byte more is `config:value` naming the bound and quoting only the
    /// start of the name.
    #[test]
    fn a_route_name_is_served_up_to_the_gateways_bound_and_refused_past_it() {
        let document = |name: &str| {
            format!(
                "format: llm.routes/1\nendpoints:\n  lab: {{base_url: http://127.0.0.1:8000/v1, protocol: responses, auth: {{kind: anonymous}}}}\nroutes:\n  {name}: lab/small\n"
            )
        };
        let longest = "r".repeat(super::MAX_ROUTE_NAME_BYTES);
        let routes = Routes::parse(&document(&longest)).unwrap();
        assert_eq!(routes.summaries().unwrap().len(), 1);
        assert_eq!(routes.relay_models().unwrap().len(), 1);
        let past = "r".repeat(super::MAX_ROUTE_NAME_BYTES + 1);
        let refused = Routes::parse(&document(&past)).unwrap_err();
        assert_eq!(refused.code(), "config:value");
        assert!(
            refused
                .message()
                .contains(&format!("at most {} bytes", super::MAX_ROUTE_NAME_BYTES)),
            "{refused}"
        );
        assert!(!refused.message().contains(&past), "{refused}");
    }

    /// The route-name bound and the `anthropic-version` a Messages route is sent are published
    /// with the numbers the code holds, on the reference page and in the specification.
    #[test]
    fn the_published_route_name_bound_and_version_are_the_codes() {
        let page = include_str!("../../../website/docs/reference/deployment-document.md");
        let spec = include_str!("../../../spec/domains/deployment.yaml");
        let bound = format!("at most {} bytes", super::MAX_ROUTE_NAME_BYTES);
        assert!(
            page.contains(&format!("A route name is {bound}")),
            "{bound}"
        );
        assert!(spec.contains(&format!("serves one of {bound}")), "{bound}");
        let version = format!("anthropic-version: {}", llm_messages::ANTHROPIC_VERSION);
        assert!(page.contains(&version), "{version}");
    }

    /// A Messages endpoint is sent llm's own `anthropic-version`, every other endpoint nothing.
    #[test]
    fn only_a_messages_endpoint_is_sent_anthropic_version() {
        let headers = super::endpoint_headers(crate::config::Wire::Messages).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].name(), llm_messages::VERSION_HEADER);
        assert_eq!(headers[0].value(), llm_messages::ANTHROPIC_VERSION);
        for wire in [crate::config::Wire::Chat, crate::config::Wire::Responses] {
            assert!(super::endpoint_headers(wire).unwrap().is_empty());
        }
    }

    #[test]
    fn a_home_secret_resolves_only_against_an_absolute_home() {
        let home = SecretFile::Home(PathBuf::from("keys/router"));
        assert_eq!(
            home.resolve(Some(Path::new("/var/lib/gateway"))),
            Some(PathBuf::from("/var/lib/gateway/keys/router"))
        );
        assert_eq!(home.resolve(Some(Path::new("relative"))), None);
        assert_eq!(home.resolve(None), None);
        let absolute = SecretFile::Absolute(PathBuf::from("/run/keys/router"));
        assert_eq!(
            absolute.resolve(None),
            Some(PathBuf::from("/run/keys/router"))
        );
    }

    #[test]
    fn an_anonymous_endpoint_is_self_hosted_and_a_plain_route_takes_llms_defaults() {
        let routes = Routes::parse(DOCUMENT).unwrap();
        let endpoint = routes.endpoints().next().unwrap();
        assert_eq!(
            endpoint.billing_kind(),
            llm_gateway::BillingKind::SelfHosted
        );
        assert_eq!(endpoint.secret(), None);
        let route = routes.routes().next().unwrap();
        assert_eq!(
            route.capabilities(),
            &llm_models::routes::default_capabilities()
        );
        assert_eq!(routes.digest().len(), 64);
    }
}
