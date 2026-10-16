//! The models of an llm `llm.routes/1` document a deployment names by its `routes` key
//! (story:routes-document; `spec/domains/deployment.yaml` `RoutesDeclaration`).
//!
//! The document is read once at start through the trusted-file reader and parsed with llm 0.6.0's
//! `Routes::parse`, which validates it strictly and performs no I/O; `b10x-llm-models` is linked
//! without its secret-source features, so llm reads no secret. What llm's API does not expose,
//! the path of an endpoint's `file:` secret, is read here from the same text with a minimal
//! reader that ignores everything else. Each route becomes a served model whose endpoint is a
//! [`HostedEndpoint`] derived from the route's endpoint: its wire, base URL, authentication and
//! credential, no static header. A request on the endpoint's own wire is relayed byte for byte to
//! its base path; a request on another wire is translated under the route's capabilities.
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
    AuthKind, BillingKind, CredentialHeader, Label, RelayModel, RouteSummary, TargetBearer,
    TargetLimits, TargetProvenance, TargetSummary, ToolCalling,
};
use llm_models::routes::{EndpointDeclaration, SecretKind};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// The provider and account every route's target is listed under.
const PROVIDER: &str = "routes";

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

    fn endpoint_of(&self, route: &Route) -> Result<&RouteEndpoint, Refusal> {
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
            };
            let served = HostedEndpoint::new(&endpoint.base_url, credential).map_err(|error| {
                value(
                    &format!("routes.{}.base_url", endpoint.name),
                    &error.to_string(),
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
    Ok(RouteEndpoint {
        name: name.to_owned(),
        base_url,
        wire: match declaration.protocol() {
            Protocol::Messages => Wire::Messages,
            Protocol::Responses => Wire::Responses,
            Protocol::ChatCompletions => Wire::Chat,
        },
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
