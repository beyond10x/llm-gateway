//! The closed TOML deployment document (row K29; llmgw `src/config.rs` at 048ebd8), mapped onto
//! the settings llm-gateway already has: `listen` onto [`GatewayConfig`], `cloud_type` onto
//! [`CloudType`], the vLLM keys onto [`VllmSettings`] and the volume keys onto
//! [`NetworkVolume`].

use crate::{
    refusal::{Refusal, StartupRefusal},
    routes::Routes,
    trusted,
};
use llm_gateway::{
    AuthKind, BillingKind, CredentialHeader, GatewayConfig, IngressAuth, ReasoningEffort,
    StaticHeader, ToolCalling,
};
use llm_runpod::{
    CloudType, ConnectorsBinding, NetworkVolume, RunpodProfile, Thinking, VllmSettings,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    time::Duration,
};

const REASONING_EFFORTS: [&str; 4] = ["low", "medium", "high", "xhigh"];
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Reads a closed enumerated value from a TOML string and nothing else. serde's derived
/// `Deserialize` for an externally tagged enum also takes a one-key table naming a unit
/// variant, so `thinking = { on = {} }` loaded as `on`; every enum of the document is read
/// through this instead, and anything but one of its strings is `config:schema`.
macro_rules! from_string_only {
    ($name:ident { $( $text:literal => $variant:ident ),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                match text.as_str() {
                    $( $text => Ok(Self::$variant), )+
                    other => Err(serde::de::Error::unknown_variant(other, &[$( $text ),+])),
                }
            }
        }
    };
}

// --- The document, exactly as written --------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    listen: SocketAddr,
    #[serde(default)]
    ingress_auth: IngressAuthDocument,
    #[serde(default)]
    owner_secret_file: Option<PathBuf>,
    /// The metrics listener's address (story:prometheus-metrics).
    #[serde(default)]
    metrics_listen: Option<SocketAddr>,
    /// The token a scrape of the metrics listener presents.
    #[serde(default)]
    metrics_bearer_file: Option<PathBuf>,
    #[serde(default)]
    providers: BTreeMap<String, ProviderDocument>,
    #[serde(default)]
    models: BTreeMap<String, ModelDocument>,
    #[serde(default)]
    hosted: BTreeMap<String, HostedDocument>,
    /// The `llm.routes/1` document whose routes are served (story:routes-document).
    #[serde(default)]
    routes: Option<PathBuf>,
}

/// `[hosted.<alias>]`: a model a hosted endpoint serves (`llm-gateway.deployment.HostedDeclaration`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedDocument {
    /// `endpoint`, the default, or `bedrock` (story:bedrock-hosted-target).
    #[serde(default)]
    kind: HostedKindDocument,
    /// Required for an endpoint, refused on a Bedrock model; the loader names a missing one as
    /// serde would.
    #[serde(default)]
    wires: Option<Vec<Wire>>,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    auth_kind: Option<AuthKindDocument>,
    /// A Bedrock model's keys, refused on an endpoint. Each is read as a string and checked as a
    /// value, so a refusal is `config:value` and quotes nothing.
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    model_id: Option<String>,
    #[serde(default)]
    credential_source: Option<String>,
    #[serde(default)]
    api: Option<String>,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    credential_file: Option<PathBuf>,
    #[serde(default)]
    credential_header: Option<String>,
    #[serde(default)]
    billing_kind: BillingKindDocument,
    #[serde(default)]
    upstream_model: Option<String>,
    #[serde(default)]
    context_window: Option<u32>,
    /// A Bedrock model's output limit, refused on an endpoint.
    #[serde(default)]
    max_output_tokens: Option<u32>,
    /// The Anthropic beta names an `InvokeModel` model takes from a client, refused on an
    /// endpoint and on a Converse model.
    #[serde(default)]
    anthropic_beta: Option<Vec<String>>,
    /// `absent` by default for an endpoint, `parsed` for a Bedrock model.
    #[serde(default)]
    tool_calling: Option<ToolCallingDocument>,
    /// Read as a string and checked as a value, so `high` is `config:value`, not
    /// `config:schema`, as a Runpod model's `reasoning_effort` is.
    #[serde(default)]
    reasoning_effort: Option<String>,
    /// `[hosted.<alias>.headers]`: static request headers, name to value.
    /// Read through [`header_table`], so a refusal of its shape quotes no value.
    #[serde(default, deserialize_with = "header_table")]
    headers: BTreeMap<String, String>,
}

/// The key under which the toml crate presents a datetime to a visitor as a one-entry table.
const TOML_DATETIME_KEY: &str = "$__toml_private_datetime";

/// `[hosted.<alias>.headers]` as a table of names to strings. serde's own refusal of a value of
/// the wrong shape quotes it (`invalid type: string "authorization: Bearer …"`), and a header
/// value may be a credential somebody put in the wrong place, so this one names the shape and
/// never the value: a table whose values are not all strings, or anything but a table.
fn header_table<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Table;

    impl<'de> serde::de::Visitor<'de> for Table {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a table of header names to string values")
        }

        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut table = BTreeMap::new();
            while let Some(name) = map.next_key::<String>()? {
                // The toml crate hands a datetime over as a one-entry table under this key, whose
                // name is a valid token: it is a datetime, not a header.
                if name == TOML_DATETIME_KEY {
                    return Err(serde::de::Error::custom(
                        "headers must be a table of header names to string values, not a datetime",
                    ));
                }
                let Some(value) = map.next_value::<Opaque>()?.0 else {
                    return Err(serde::de::Error::custom(
                        "every value of headers must be a string",
                    ));
                };
                table.insert(name, value);
            }
            Ok(table)
        }

        fn visit_str<E: serde::de::Error>(self, _text: &str) -> Result<Self::Value, E> {
            Err(not_a_table("a string"))
        }

        fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Self::Value, E> {
            Err(not_a_table("an integer"))
        }

        fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Self::Value, E> {
            Err(not_a_table("an integer"))
        }

        fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
            Err(not_a_table("a float"))
        }

        fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Self::Value, E> {
            Err(not_a_table("a boolean"))
        }
    }

    /// The refusal of a `headers` of the wrong shape, naming the shape and never the value.
    fn not_a_table<E: serde::de::Error>(shape: &str) -> E {
        E::custom(format!(
            "headers must be a table of header names to string values, not {shape}"
        ))
    }

    /// A value read whole and kept only when it is a string, so nothing else is quoted.
    struct Opaque(Option<String>);

    impl<'de> serde::Deserialize<'de> for Opaque {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Any;

            impl<'de> serde::de::Visitor<'de> for Any {
                type Value = Opaque;

                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a string")
                }

                fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<Opaque, E> {
                    Ok(Opaque(Some(text.to_owned())))
                }

                fn visit_string<E: serde::de::Error>(self, text: String) -> Result<Opaque, E> {
                    Ok(Opaque(Some(text)))
                }

                fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Opaque, E> {
                    Ok(Opaque(None))
                }

                fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Opaque, E> {
                    Ok(Opaque(None))
                }

                fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Opaque, E> {
                    Ok(Opaque(None))
                }

                fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Opaque, E> {
                    Ok(Opaque(None))
                }

                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Opaque, A::Error> {
                    while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                    Ok(Opaque(None))
                }

                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Opaque, A::Error> {
                    while map
                        .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                        .is_some()
                    {}
                    Ok(Opaque(None))
                }
            }

            deserializer.deserialize_any(Any)
        }
    }

    deserializer.deserialize_map(Table)
}

#[derive(Default, PartialEq, Eq)]
enum HostedKindDocument {
    #[default]
    Endpoint,
    Bedrock,
}

from_string_only!(HostedKindDocument { "endpoint" => Endpoint, "bedrock" => Bedrock });

enum AuthKindDocument {
    Anonymous,
    Bearer,
    ApiKey,
}

from_string_only!(AuthKindDocument {
    "anonymous" => Anonymous, "bearer" => Bearer, "api-key" => ApiKey
});

#[derive(Default)]
enum BillingKindDocument {
    #[default]
    Metered,
    Subscription,
    SelfHosted,
}

from_string_only!(BillingKindDocument {
    "metered" => Metered, "subscription" => Subscription, "self-hosted" => SelfHosted
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderDocument {
    kind: ProviderKind,
    #[serde(default)]
    cloud_type: CloudTypeDocument,
    #[serde(default)]
    connectors: Option<ConnectorsDocument>,
    #[serde(default)]
    daily_cap_usd: Option<f64>,
    #[serde(default)]
    total_cap_usd: Option<f64>,
}

/// `[providers.<name>.connectors]` (`llm-gateway.deployment.ConnectorsDeclaration`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorsDocument {
    executable: PathBuf,
    adapter: String,
    connection: String,
    work_directory: PathBuf,
    #[serde(default = "default_connectors_timeout_seconds")]
    timeout_seconds: u64,
}

fn default_connectors_timeout_seconds() -> u64 {
    60
}

#[derive(Default)]
enum CloudTypeDocument {
    #[default]
    Secure,
    Community,
}

from_string_only!(CloudTypeDocument { "SECURE" => Secure, "COMMUNITY" => Community });

/// `ingress_auth` (`llm-gateway.deployment.IngressAuth`).
#[derive(Default)]
enum IngressAuthDocument {
    #[default]
    Bearer,
    None,
}

from_string_only!(IngressAuthDocument { "bearer" => Bearer, "none" => None });

#[derive(Default)]
enum ThinkingDocument {
    On,
    #[default]
    Off,
}

from_string_only!(ThinkingDocument { "on" => On, "off" => Off });

#[derive(Default, Clone, Copy)]
enum ToolCallingDocument {
    Parsed,
    /// The default: a model that says nothing is served no tool request.
    #[default]
    Absent,
}

from_string_only!(ToolCallingDocument { "parsed" => Parsed, "absent" => Absent });

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelDocument {
    provider: String,
    wires: Vec<Wire>,
    context_window: u32,
    hf_model: String,
    image: String,
    /// The flat declaration of the one profile `default`; refused beside `profiles`.
    #[serde(default)]
    gpu_types: Option<Vec<String>>,
    #[serde(default)]
    max_model_len: Option<u32>,
    #[serde(default)]
    max_num_seqs: Option<u32>,
    #[serde(default)]
    gpu_util: Option<f64>,
    #[serde(default)]
    profiles: Option<Vec<ProfileDocument>>,
    #[serde(default = "default_disk_gb")]
    disk_gb: u32,
    #[serde(default)]
    thinking: ThinkingDocument,
    #[serde(default)]
    reasoning_effort: Option<String>,
    #[serde(default)]
    sampling: Option<String>,
    #[serde(default)]
    extra_vllm_args: Option<Vec<String>>,
    #[serde(default)]
    network_volume_id: Option<String>,
    #[serde(default = "default_volume_mount_path")]
    volume_mount_path: String,
    #[serde(default)]
    data_center_ids: Vec<String>,
    #[serde(default = "default_idle_timeout_minutes")]
    idle_timeout_minutes: u64,
    #[serde(default = "default_start_wait_seconds")]
    start_wait_seconds: u64,
    /// Defaults to `start_wait_seconds` (row K28).
    #[serde(default)]
    request_hold_seconds: Option<u64>,
    #[serde(default)]
    vllm_api_key_file: Option<PathBuf>,
    #[serde(default)]
    tool_calling: ToolCallingDocument,
    #[serde(default)]
    price_per_hour_usd: Option<f64>,
}

/// One `[[models.<alias>.profiles]]` entry, or the flat keys as the one profile `default`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileDocument {
    name: String,
    gpu_types: Vec<String>,
    max_model_len: u32,
    #[serde(default = "default_max_num_seqs")]
    max_num_seqs: u32,
    #[serde(default = "default_gpu_util")]
    gpu_util: f64,
    #[serde(default)]
    extra_vllm_args: Vec<String>,
}

/// The most profiles one model may declare.
const MAX_PROFILES: usize = 8;

fn default_max_num_seqs() -> u32 {
    8
}

fn default_gpu_util() -> f64 {
    0.90
}

fn default_disk_gb() -> u32 {
    80
}

fn default_volume_mount_path() -> String {
    "/workspace".to_string()
}

fn default_idle_timeout_minutes() -> u64 {
    30
}

fn default_start_wait_seconds() -> u64 {
    600
}

// --- The loaded deployment -------------------------------------------------------------------

/// `providers.<name>.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    RunpodVllm,
}

from_string_only!(ProviderKind { "runpod-vllm" => RunpodVllm });

/// A wire a model is served on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Wire {
    Chat,
    Responses,
    Messages,
}

from_string_only!(Wire { "chat" => Chat, "responses" => Responses, "messages" => Messages });

impl Wire {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Messages => "messages",
        }
    }
}

/// `[providers.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub kind: ProviderKind,
    pub cloud_type: CloudType,
    /// The connectors connection the provider's pods are reached through, when it declares
    /// one; `None` leaves its models answered `target-unavailable`.
    pub connectors: Option<ConnectorsBinding>,
    /// The account's daily spend ceiling in micro-USD (`daily_cap_usd`); `None` is none.
    pub daily_cap_micro_usd: Option<u64>,
    /// The account's total spend ceiling in micro-USD (`total_cap_usd`); `None` is none.
    pub total_cap_micro_usd: Option<u64>,
}

/// `[models.<alias>]`, with every default applied and every rule checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub provider: String,
    pub wires: Vec<Wire>,
    pub context_window: u32,
    pub hf_model: String,
    pub image: String,
    /// The first profile's GPU types.
    pub gpu_types: Vec<String>,
    pub disk_gb: u32,
    /// The first profile's vLLM settings.
    pub vllm: VllmSettings,
    /// The first profile's name; `default` for the flat declaration.
    pub profile: String,
    /// The profiles after the first, in order. Each shares the model's thinking, reasoning
    /// effort and sampling.
    pub fallbacks: Vec<RunpodProfile>,
    pub cache: Option<NetworkVolume>,
    pub data_center_ids: Vec<String>,
    pub idle_timeout_minutes: u64,
    /// The pod's startup deadline.
    pub start_wait_seconds: u64,
    /// How long one request waits for the pod: its own setting, at most `start_wait_seconds`,
    /// which it defaults to (row K28).
    pub request_hold_seconds: u64,
    /// The file holding the key this model's pod's vLLM server expects (row B9). Only the path
    /// is loaded here; [`crate::vllm_keys`] reads it.
    pub vllm_api_key_file: Option<PathBuf>,
    /// Whether the model's pod parses tool calls; `absent` unless the document says
    /// `parsed`. A request offering tools to an `Absent` model is refused before a pod is
    /// asked for.
    pub tool_calling: ToolCalling,
    /// The most the model's pod costs per hour on any of its GPU types, in micro-USD
    /// (`price_per_hour_usd`), which the spend check prices its pods by. Required once a
    /// provider declares a spend ceiling.
    pub price_per_hour_micro_usd: Option<u64>,
}

/// `[hosted.<alias>]`, with every default applied and every rule checked: a model a hosted
/// endpoint serves (`llm-gateway.upstream.HostedEndpoint`).
#[derive(Debug, Clone, PartialEq)]
pub struct Hosted {
    pub wires: Vec<Wire>,
    /// `http://` or `https://`, an authority and an optional base path, as written. Never
    /// rendered by the route inventory.
    pub base_url: String,
    pub auth_kind: AuthKind,
    /// The file holding the credential; absent exactly when `auth_kind` is `Anonymous`. Only
    /// the path is loaded here; the composition reads it.
    pub credential_file: Option<PathBuf>,
    /// The header an `ApiKey` endpoint receives the credential in; present exactly then.
    pub credential_header: Option<CredentialHeader>,
    pub billing_kind: BillingKind,
    /// The model name the endpoint expects: the alias unless the declaration names another.
    pub upstream_model: String,
    /// What the public routes publish, when declared.
    pub context_window: Option<u32>,
    pub tool_calling: ToolCalling,
    /// The effort a messages-wire request naming none is given, when declared.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// The static headers every request to the endpoint carries, in name order
    /// (story:hosted-static-headers).
    pub headers: Vec<StaticHeader>,
}

/// `[hosted.<alias>] kind = "bedrock"`, with every default applied and every rule checked: a
/// model Amazon Bedrock serves (`llm-gateway.upstream.HostedEndpoint`, story:bedrock-hosted-target).
/// None of its values is rendered by the route inventory or written to the log.
#[derive(Debug, Clone, PartialEq)]
pub struct Bedrock {
    /// The AWS Region the request is signed for, and the default endpoint's.
    pub region: String,
    /// The model id, inference profile id or ARN Bedrock is asked for.
    pub model_id: String,
    /// The Bedrock Runtime operation: `anthropic-invoke` by default for an Anthropic model id,
    /// `converse` otherwise.
    pub api: llm_bedrock::Api,
    /// Replaces `https://bedrock-runtime.<region>.amazonaws.com` when declared.
    pub endpoint: Option<url::Url>,
    pub billing_kind: BillingKind,
    pub context_window: Option<u32>,
    /// The most output one turn may ask for, within the window; the window when undeclared.
    pub max_output_tokens: Option<u32>,
    /// `parsed` unless the document says `absent`.
    pub tool_calling: ToolCalling,
    /// The Anthropic beta names a client's `anthropic-beta` header may carry to this model, in
    /// the document's order: each `llm_core::anthropic_beta_name`, distinct, at most
    /// `llm_core::MAX_ANTHROPIC_BETAS`. Empty unless declared, and always empty for `converse`.
    pub anthropic_beta: Vec<String>,
}

/// A loaded and validated deployment document.
#[derive(Debug, Clone)]
pub struct Deployment {
    pub gateway: GatewayConfig,
    /// The owner secret's file: present whenever `gateway.ingress_auth` is `Bearer`, and
    /// optional when it is `None` (story:ingress-auth-optional).
    pub owner_secret_file: Option<PathBuf>,
    /// Where the metrics listener binds, serving `GET /metrics` without the owner credential;
    /// never `gateway.bind`. `None` binds none.
    pub metrics_listen: Option<SocketAddr>,
    /// The file of the token a scrape must present; only with `metrics_listen`.
    pub metrics_bearer_file: Option<PathBuf>,
    pub providers: BTreeMap<String, Provider>,
    pub models: BTreeMap<String, Model>,
    /// The models hosted endpoints serve, by alias. No alias is in both tables.
    pub hosted: BTreeMap<String, Hosted>,
    /// The models Amazon Bedrock serves, by alias: the `[hosted.*]` entries of
    /// `kind = "bedrock"`. No alias is in two tables.
    pub bedrock: BTreeMap<String, Bedrock>,
    /// The routes document `routes` names, as written; only the path is loaded by [`parse`].
    pub routes_file: Option<PathBuf>,
    /// The routes of that document once [`load`] (or [`Deployment::with_routes`]) has read it;
    /// empty before, and when the document names none. No route name is a `models` or `hosted`
    /// alias.
    pub routes: Routes,
    /// SHA-256 of the exact document bytes read, as 64 lowercase hex characters.
    pub digest: String,
}

impl Deployment {
    /// The deployment serving `routes` beside its own models.
    ///
    /// # Errors
    /// `config:value` naming `routes.<route>` for a route name that is a `[models.*]` or
    /// `[hosted.*]` alias, and naming `models` when no model of any kind is left to serve.
    pub fn with_routes(mut self, routes: Routes) -> Result<Self, Refusal> {
        for route in routes.routes() {
            ensure(
                !self.models.contains_key(route.name())
                    && !self.hosted.contains_key(route.name())
                    && !self.bedrock.contains_key(route.name()),
                &format!("routes.{}", route.name()),
                "names an alias [models.*] or [hosted.*] already declares",
            )?;
        }
        ensure(
            !(self.models.is_empty()
                && self.hosted.is_empty()
                && self.bedrock.is_empty()
                && routes.is_empty()),
            "models",
            "at least one model, of models, hosted or the routes document, is required",
        )?;
        self.routes = routes;
        Ok(self)
    }
}

/// SHA-256 of `bytes`, as 64 lowercase hex characters.
pub(crate) fn hex_digest(bytes: &[u8]) -> String {
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        digest.push(char::from(HEX[usize::from(byte >> 4)]));
        digest.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    digest
}

/// Reads `path` through the trusted-file reader and loads the closed document, then the routes
/// document it names, if any (story:routes-document).
///
/// # Errors
/// A `config:*` [`Refusal`]: a trusted-file rule, `config:schema` for anything outside the
/// closed document, or `config:value` for a value outside its rule; for the routes document, a
/// trusted-file rule or `config:value`, each message starting `routes`.
pub fn load(path: &Path) -> Result<Deployment, Refusal> {
    let text = trusted::read(path, &trusted::CONFIG)?;
    let deployment = parse(&text)?;
    match deployment.routes_file.clone() {
        Some(file) => {
            let routes = crate::routes::load(&file)?;
            deployment.with_routes(routes)
        }
        None => Ok(deployment),
    }
}

fn schema_refusal(text: &str, error: &toml::de::Error) -> Refusal {
    let message = error.message();
    match error.span() {
        Some(span) => {
            // Counted over bytes: the span is a byte offset, which need not fall on a character
            // boundary of multi-byte text.
            let end = span.start.min(text.len());
            // n newlines split the bytes into n + 1 pieces, which is the 1-based line number.
            let line = text.as_bytes()[..end].split(|byte| *byte == b'\n').count();
            Refusal::new(
                StartupRefusal::ConfigSchema,
                format!("line {line}: {message}"),
            )
        }
        None => Refusal::new(StartupRefusal::ConfigSchema, message),
    }
}

/// Loads the closed document from its text, as [`load`] does once the trusted-file reader has
/// read it. No file the document names is read.
///
/// # Errors
/// `config:schema` for anything outside the closed document, or `config:value` for a value
/// outside its rule.
pub fn parse(text: &str) -> Result<Deployment, Refusal> {
    let document: Document = toml::from_str(text).map_err(|error| schema_refusal(text, &error))?;
    let digest = hex_digest(text.as_bytes());
    if document.models.is_empty() && document.hosted.is_empty() && document.routes.is_none() {
        return Err(value(
            "models",
            "at least one model, of models, hosted or the routes document, is required",
        ));
    }
    let (providers, metered) = providers(document.providers)?;
    let mut models = BTreeMap::new();
    for (alias, model) in document.models {
        let at = format!("models.{alias}");
        closed_name(&at, &alias)?;
        if !providers.contains_key(&model.provider) {
            return Err(value(
                &format!("{at}.provider"),
                "must name a declared provider",
            ));
        }
        let model = validate_model(&at, model)?;
        ensure(
            metered == 0 || model.price_per_hour_micro_usd.is_some(),
            &format!("{at}.price_per_hour_usd"),
            "is required once a provider declares a spend ceiling",
        )?;
        models.insert(alias, model);
    }
    let mut hosted = BTreeMap::new();
    let mut bedrock = BTreeMap::new();
    for (alias, declared) in document.hosted {
        let at = format!("hosted.{alias}");
        closed_name(&at, &alias)?;
        ensure(
            !models.contains_key(&alias),
            &at,
            "names an alias models already declares",
        )?;
        if declared.kind == HostedKindDocument::Bedrock {
            let loaded = validate_bedrock(&at, declared)?;
            bedrock.insert(alias, loaded);
        } else {
            let loaded = validate_hosted(&at, &alias, declared)?;
            hosted.insert(alias, loaded);
        }
    }
    let ingress_auth = match document.ingress_auth {
        IngressAuthDocument::Bearer => IngressAuth::Bearer,
        IngressAuthDocument::None => IngressAuth::None,
    };
    ensure(
        ingress_auth == IngressAuth::None || document.owner_secret_file.is_some(),
        "owner_secret_file",
        "is required unless ingress_auth is none",
    )?;
    ensure(
        document.metrics_listen != Some(document.listen),
        "metrics_listen",
        "must differ from listen",
    )?;
    ensure(
        document.metrics_bearer_file.is_none() || document.metrics_listen.is_some(),
        "metrics_bearer_file",
        "is allowed only with metrics_listen",
    )?;
    let mut gateway = GatewayConfig::new(document.listen);
    gateway.ingress_auth = ingress_auth;
    Ok(Deployment {
        gateway,
        owner_secret_file: document.owner_secret_file,
        metrics_listen: document.metrics_listen,
        metrics_bearer_file: document.metrics_bearer_file,
        providers,
        models,
        hosted,
        bedrock,
        routes_file: document.routes,
        routes: Routes::default(),
        digest,
    })
}

/// `[providers.<name>]`, checked: the providers, and how many of them declare a spend ceiling
/// (at most one).
fn providers(
    declared: BTreeMap<String, ProviderDocument>,
) -> Result<(BTreeMap<String, Provider>, usize), Refusal> {
    let mut providers = BTreeMap::new();
    for (name, provider) in declared {
        closed_name(&format!("providers.{name}"), &name)?;
        let cloud_type = match provider.cloud_type {
            CloudTypeDocument::Secure => CloudType::Secure,
            CloudTypeDocument::Community => CloudType::Community,
        };
        let connectors = provider
            .connectors
            .map(|table| connectors_binding(&format!("providers.{name}.connectors"), table))
            .transpose()?;
        let daily_cap_micro_usd = provider
            .daily_cap_usd
            .map(|usd| amount(&format!("providers.{name}.daily_cap_usd"), usd, MAX_CAP_USD))
            .transpose()?;
        let total_cap_micro_usd = provider
            .total_cap_usd
            .map(|usd| amount(&format!("providers.{name}.total_cap_usd"), usd, MAX_CAP_USD))
            .transpose()?;
        providers.insert(
            name,
            Provider {
                kind: provider.kind,
                cloud_type,
                connectors,
                daily_cap_micro_usd,
                total_cap_micro_usd,
            },
        );
    }
    ensure(
        providers
            .values()
            .filter(|provider| provider.connectors.is_some())
            .count()
            <= 1,
        "providers",
        "may declare at most one connectors connection",
    )?;
    let metered = providers
        .values()
        .filter(|provider| {
            provider.daily_cap_micro_usd.is_some() || provider.total_cap_micro_usd.is_some()
        })
        .count();
    ensure(
        metered <= 1,
        "providers",
        "may declare a spend ceiling on at most one provider",
    )?;
    Ok((providers, metered))
}

/// `[hosted.<alias>]`, checked (`llm-gateway.deployment.HostedDeclaration`).
/// Refuses on an endpoint each key only a Bedrock model has.
fn refuse_bedrock_keys(at: &str, declared: &HostedDocument) -> Result<(), Refusal> {
    for (name, present) in [
        ("region", declared.region.is_some()),
        ("model_id", declared.model_id.is_some()),
        ("credential_source", declared.credential_source.is_some()),
        ("api", declared.api.is_some()),
        ("endpoint", declared.endpoint.is_some()),
        ("max_output_tokens", declared.max_output_tokens.is_some()),
        ("anthropic_beta", declared.anthropic_beta.is_some()),
    ] {
        ensure(
            !present,
            &format!("{at}.{name}"),
            "is a key of kind = \"bedrock\" only",
        )?;
    }
    Ok(())
}

fn validate_hosted(at: &str, alias: &str, declared: HostedDocument) -> Result<Hosted, Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    // A required key left out is outside the closed document, as serde would have said.
    let missing = |name: &str| {
        Refusal::new(
            StartupRefusal::ConfigSchema,
            format!("{at}: missing field `{name}`"),
        )
    };
    refuse_bedrock_keys(at, &declared)?;
    let wires = declared.wires.ok_or_else(|| missing("wires"))?;
    let base_url = declared.base_url.ok_or_else(|| missing("base_url"))?;
    let auth_kind = declared.auth_kind.ok_or_else(|| missing("auth_kind"))?;
    ensure(!wires.is_empty(), &key("wires"), "must not be empty")?;
    let unique: BTreeSet<Wire> = wires.iter().copied().collect();
    ensure(
        unique.len() == wires.len(),
        &key("wires"),
        "must not repeat a wire",
    )?;
    let auth_kind = match auth_kind {
        AuthKindDocument::Anonymous => AuthKind::Anonymous,
        AuthKindDocument::Bearer => AuthKind::Bearer,
        AuthKindDocument::ApiKey => AuthKind::ApiKey,
    };
    match crate::hosted::HostedEndpoint::check(&base_url, auth_kind) {
        Ok(_) => {}
        Err(crate::hosted::BaseUrlError::Malformed) => {
            return Err(value(
                &key("base_url"),
                "must be http:// or https://, a host[:port] of at most 256 printable ASCII \
                 characters and an optional base path of at most 32 segments and 256 bytes, each \
                 segment one or more of A-Z a-z 0-9 - . _ ~ and neither . nor .., with no query, \
                 fragment or user information",
            ));
        }
        Err(crate::hosted::BaseUrlError::CleartextCredential) => {
            return Err(value(
                &key("base_url"),
                "must be https:// for a credential sent to a host that is not loopback \
                 (127.0.0.0/8, ::1 or localhost): a credential is never sent in cleartext",
            ));
        }
    }
    ensure(
        declared.credential_file.is_some() != (auth_kind == AuthKind::Anonymous),
        &key("credential_file"),
        "is required unless auth_kind is anonymous, and absent when it is",
    )?;
    ensure(
        declared.credential_header.is_some() == (auth_kind == AuthKind::ApiKey),
        &key("credential_header"),
        "is required when auth_kind is api-key, and absent otherwise",
    )?;
    let credential_header = declared
        .credential_header
        .as_deref()
        .map(|name| {
            CredentialHeader::new(name)
                .map_err(|error| value(&key("credential_header"), &error.to_string()))
        })
        .transpose()?;
    let upstream_model = declared.upstream_model.unwrap_or_else(|| alias.to_owned());
    bounded_text(&key("upstream_model"), &upstream_model, 256)?;
    if let Some(window) = declared.context_window {
        within(at, "context_window", &window, &(4_096..=2_000_000))?;
    }
    let reasoning_effort = declared
        .reasoning_effort
        .as_deref()
        .map(|effort| {
            ReasoningEffort::from_name(effort).ok_or_else(|| {
                value(
                    &key("reasoning_effort"),
                    "must be one of low, medium, xhigh",
                )
            })
        })
        .transpose()?;
    let headers = static_headers(&key("headers"), &declared.headers)?;
    crate::hosted::check_headers(&headers, credential_header.as_ref())
        .map_err(|error| value(&key("headers"), &error.to_string()))?;
    Ok(Hosted {
        headers,
        wires,
        base_url,
        auth_kind,
        credential_file: declared.credential_file,
        credential_header,
        billing_kind: billing_kind(&declared.billing_kind),
        upstream_model,
        context_window: declared.context_window,
        tool_calling: tool_calling(declared.tool_calling.unwrap_or_default()),
        reasoning_effort,
    })
}

const fn tool_calling(declared: ToolCallingDocument) -> ToolCalling {
    match declared {
        ToolCallingDocument::Parsed => ToolCalling::Parsed,
        ToolCallingDocument::Absent => ToolCalling::Absent,
    }
}

const fn billing_kind(declared: &BillingKindDocument) -> BillingKind {
    match declared {
        BillingKindDocument::Metered => BillingKind::Metered,
        BillingKindDocument::Subscription => BillingKind::Subscription,
        BillingKindDocument::SelfHosted => BillingKind::SelfHosted,
    }
}

/// `[hosted.<alias>] anthropic_beta`, checked (story:bedrock-llm-0-13): empty when undeclared;
/// otherwise an `anthropic-invoke` model's, at most `llm_core::MAX_ANTHROPIC_BETAS` distinct
/// names, each by `llm_core::anthropic_beta_name`. A refusal names the key, never a name.
fn validate_betas(
    at: &str,
    api: llm_bedrock::Api,
    declared: Option<Vec<String>>,
) -> Result<Vec<String>, Refusal> {
    let betas = declared.unwrap_or_default();
    if betas.is_empty() {
        return Ok(betas);
    }
    let at = format!("{at}.anthropic_beta");
    ensure(
        api == llm_bedrock::Api::AnthropicInvoke,
        &at,
        "is a key of api = \"anthropic-invoke\" only: Converse carries no beta",
    )?;
    ensure(
        betas.len() <= llm_core::MAX_ANTHROPIC_BETAS,
        &at,
        &format!("names at most {} betas", llm_core::MAX_ANTHROPIC_BETAS),
    )?;
    ensure(
        betas.iter().all(|name| llm_core::anthropic_beta_name(name)),
        &at,
        &format!(
            "names each beta with 1..={} ASCII letters, digits, '-', '_' or '.'",
            llm_core::MAX_ANTHROPIC_BETA_BYTES
        ),
    )?;
    let distinct: std::collections::BTreeSet<&String> = betas.iter().collect();
    ensure(distinct.len() == betas.len(), &at, "names each beta once")?;
    Ok(betas)
}

/// `[hosted.<alias>] kind = "bedrock"`, checked (`llm-gateway.deployment.HostedDeclaration`,
/// story:bedrock-hosted-target). The region, the model id and the endpoint are each checked by
/// the rule llm's Bedrock client applies, and a refusal names the key and never the value.
fn validate_bedrock(at: &str, declared: HostedDocument) -> Result<Bedrock, Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    for (name, present) in [
        ("wires", declared.wires.is_some()),
        ("base_url", declared.base_url.is_some()),
        ("auth_kind", declared.auth_kind.is_some()),
        ("credential_file", declared.credential_file.is_some()),
        ("credential_header", declared.credential_header.is_some()),
        ("upstream_model", declared.upstream_model.is_some()),
        ("reasoning_effort", declared.reasoning_effort.is_some()),
        ("headers", !declared.headers.is_empty()),
    ] {
        ensure(
            !present,
            &key(name),
            "is not a key of kind = \"bedrock\": Bedrock is reached by region and model_id and \
             signed with the container credentials, and takes no reasoning effort",
        )?;
    }
    let required = |name: &str, given: Option<String>| {
        given.ok_or_else(|| value(&key(name), "is required for kind = \"bedrock\""))
    };
    let region = required("region", declared.region)?;
    let model_id = required("model_id", declared.model_id)?;
    let source = required("credential_source", declared.credential_source)?;
    ensure(
        source == "container",
        &key("credential_source"),
        "must be container: the credentials come from the container credentials endpoint",
    )?;
    let api = match declared.api.as_deref() {
        None => crate::bedrock::default_api(&model_id),
        Some("anthropic-invoke") => llm_bedrock::Api::AnthropicInvoke,
        Some("converse") => llm_bedrock::Api::Converse,
        Some(_) => return Err(value(&key("api"), "must be anthropic-invoke or converse")),
    };
    ensure(
        crate::bedrock::region_admitted(&region),
        &key("region"),
        "must be an AWS Region name: two or more parts of a-z and 0-9 joined by -, at most 32 \
         characters",
    )?;
    ensure(
        crate::bedrock::model_id_admitted(&model_id),
        &key("model_id"),
        "must be 1..=2048 printable ASCII characters, not only dots",
    )?;
    let endpoint = declared
        .endpoint
        .as_deref()
        .map(|text| {
            crate::bedrock::endpoint_admitted(text).ok_or_else(|| {
                value(
                    &key("endpoint"),
                    "must be an https origin, or an http origin on a loopback address \
                     (127.0.0.0/8, [::1]), with no user information, path, query or fragment",
                )
            })
        })
        .transpose()?;
    if let Some(window) = declared.context_window {
        within(at, "context_window", &window, &(4_096..=2_000_000))?;
    }
    if let Some(output) = declared.max_output_tokens {
        let window = declared
            .context_window
            .unwrap_or(crate::bedrock::DEFAULT_CONTEXT_WINDOW);
        within(at, "max_output_tokens", &output, &(1..=window))?;
    }
    let anthropic_beta = validate_betas(at, api, declared.anthropic_beta)?;
    Ok(Bedrock {
        region,
        model_id,
        api,
        endpoint,
        billing_kind: billing_kind(&declared.billing_kind),
        context_window: declared.context_window,
        max_output_tokens: declared.max_output_tokens,
        tool_calling: tool_calling(declared.tool_calling.unwrap_or(ToolCallingDocument::Parsed)),
        anthropic_beta,
    })
}

/// `[hosted.<alias>.headers]`, each checked as a [`StaticHeader`] and sorted by its lowercase
/// name. The refusal names the key and the rule, never a name or a value.
fn static_headers(
    key: &str,
    declared: &BTreeMap<String, String>,
) -> Result<Vec<StaticHeader>, Refusal> {
    let mut headers = declared
        .iter()
        .map(|(name, text)| {
            StaticHeader::new(name, text).map_err(|error| value(key, &error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    headers.sort_by(|left, right| left.name().cmp(right.name()));
    Ok(headers)
}

fn value(key: &str, rule: &str) -> Refusal {
    Refusal::new(StartupRefusal::ConfigValue, format!("{key} {rule}"))
}

fn ensure(holds: bool, key: &str, rule: &str) -> Result<(), Refusal> {
    if holds { Ok(()) } else { Err(value(key, rule)) }
}

fn within<T: PartialOrd + std::fmt::Display>(
    at: &str,
    key: &str,
    actual: &T,
    range: &RangeInclusive<T>,
) -> Result<(), Refusal> {
    ensure(
        range.contains(actual),
        &format!("{at}.{key}"),
        &format!("must be within {}..={}", range.start(), range.end()),
    )
}

/// A provider name or model alias: 1..=64 of `a-z 0-9 - _ .` without a leading dash.
fn closed_name(key: &str, name: &str) -> Result<(), Refusal> {
    ensure(
        (1..=64).contains(&name.len())
            && !name.starts_with('-')
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            }),
        key,
        "must be 1..=64 lowercase URL-safe ASCII characters without a leading dash",
    )
}

/// An absolute path of 1..=1024 bytes without a control character.
fn absolute_path(key: &str, path: &Path) -> Result<(), Refusal> {
    let bytes = path.as_os_str().as_encoded_bytes();
    ensure(
        path.is_absolute()
            && (1..=1024).contains(&bytes.len())
            && !bytes.iter().any(u8::is_ascii_control),
        key,
        "must be an absolute path of at most 1024 bytes without a control character",
    )
}

/// One argument of the connectors CLI: 1..=128 printable ASCII without a space, never read as
/// an option.
fn cli_argument(key: &str, value: &str) -> Result<(), Refusal> {
    ensure(
        (1..=128).contains(&value.len())
            && !value.starts_with('-')
            && value.bytes().all(|byte| byte.is_ascii_graphic()),
        key,
        "must be 1..=128 printable ASCII characters without a space or a leading dash",
    )
}

/// `[providers.<name>.connectors]`, checked, as the transport's binding.
fn connectors_binding(at: &str, table: ConnectorsDocument) -> Result<ConnectorsBinding, Refusal> {
    absolute_path(&format!("{at}.executable"), &table.executable)?;
    absolute_path(&format!("{at}.work_directory"), &table.work_directory)?;
    cli_argument(&format!("{at}.adapter"), &table.adapter)?;
    cli_argument(&format!("{at}.connection"), &table.connection)?;
    within(at, "timeout_seconds", &table.timeout_seconds, &(1..=600))?;
    Ok(ConnectorsBinding {
        executable: table.executable,
        adapter: table.adapter,
        connection: table.connection,
        work_directory: table.work_directory,
        timeout: Duration::from_secs(table.timeout_seconds),
    })
}

fn bounded_text(key: &str, value: &str, limit: usize) -> Result<(), Refusal> {
    ensure(
        (1..=limit).contains(&value.len())
            && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte)),
        key,
        &format!("must be 1..={limit} printable ASCII characters"),
    )
}

fn validate_model(at: &str, mut model: ModelDocument) -> Result<Model, Refusal> {
    let profiles = declared_profiles(at, &mut model)?;
    check_serving(at, &model)?;
    check_vllm(at, &model, &profiles)?;
    check_placement(at, &model, &profiles)?;
    check_profile_conflict(at, &profiles)?;
    let price = model
        .price_per_hour_usd
        .map(|usd| {
            amount(
                &format!("{at}.price_per_hour_usd"),
                usd,
                MAX_PRICE_PER_HOUR_USD,
            )
        })
        .transpose()?;
    Ok(Model {
        price_per_hour_micro_usd: price,
        ..into_model(model, profiles)
    })
}

/// The model's profiles, each with the key its values are named under: the `profiles` list,
/// or the flat keys as the one profile `default`, named under the model's own key.
fn declared_profiles(
    at: &str,
    model: &mut ModelDocument,
) -> Result<Vec<(String, ProfileDocument)>, Refusal> {
    let Some(profiles) = model.profiles.take() else {
        let (Some(gpu_types), Some(max_model_len)) = (model.gpu_types.take(), model.max_model_len)
        else {
            return Err(Refusal::new(
                StartupRefusal::ConfigSchema,
                format!("{at}: missing gpu_types and max_model_len, or profiles"),
            ));
        };
        return Ok(vec![(
            at.to_owned(),
            ProfileDocument {
                name: "default".to_owned(),
                gpu_types,
                max_model_len,
                max_num_seqs: model.max_num_seqs.unwrap_or_else(default_max_num_seqs),
                gpu_util: model.gpu_util.unwrap_or_else(default_gpu_util),
                extra_vllm_args: model.extra_vllm_args.take().unwrap_or_default(),
            },
        )]);
    };
    let key = format!("{at}.profiles");
    let flat = model.gpu_types.is_some()
        || model.max_model_len.is_some()
        || model.max_num_seqs.is_some()
        || model.gpu_util.is_some()
        || model.extra_vllm_args.is_some();
    ensure(
        !flat,
        &key,
        "replaces gpu_types, max_model_len, max_num_seqs, gpu_util and extra_vllm_args: declare them in each profile",
    )?;
    ensure(
        (1..=MAX_PROFILES).contains(&profiles.len()),
        &key,
        &format!("must hold 1..={MAX_PROFILES} profiles"),
    )?;
    Ok(profiles
        .into_iter()
        .enumerate()
        .map(|(index, profile)| (format!("{key}[{index}]"), profile))
        .collect())
}

/// No two profiles of a model share a name or a GPU type, so a pod's GPU type names its
/// profile.
fn check_profile_conflict(at: &str, profiles: &[(String, ProfileDocument)]) -> Result<(), Refusal> {
    let mut names = BTreeSet::new();
    let mut gpus: BTreeSet<&str> = BTreeSet::new();
    for (_, profile) in profiles {
        let own: BTreeSet<&str> = profile.gpu_types.iter().map(String::as_str).collect();
        ensure(
            names.insert(profile.name.as_str()) && gpus.is_disjoint(&own),
            &format!("{at}.profiles"),
            "profile-conflict: two profiles share a name or a GPU type",
        )?;
        gpus.extend(own);
    }
    Ok(())
}

/// The most a spend ceiling may be, in USD.
const MAX_CAP_USD: f64 = 1_000_000.0;
/// The most a model's price per hour may be, in USD.
const MAX_PRICE_PER_HOUR_USD: f64 = 1_000.0;

/// A declared USD amount in micro-USD: finite, above 0, at most `max` and at least one
/// micro-USD, or `config:value` naming `key`. The value is never quoted.
fn amount(key: &str, usd: f64, max: f64) -> Result<u64, Refusal> {
    let rule =
        format!("must be a finite amount above 0 and at most {max} USD, of at least 0.000001");
    if usd.is_finite() && usd <= max {
        llm_runpod::micro_usd(usd).ok_or_else(|| value(key, &rule))
    } else {
        Err(value(key, &rule))
    }
}

/// What the gateway serves and how long it holds a request.
fn check_serving(at: &str, model: &ModelDocument) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    ensure(!model.wires.is_empty(), &key("wires"), "must not be empty")?;
    let unique: BTreeSet<Wire> = model.wires.iter().copied().collect();
    ensure(
        unique.len() == model.wires.len(),
        &key("wires"),
        "must not repeat a wire",
    )?;
    within(
        at,
        "context_window",
        &model.context_window,
        &(4_096..=2_000_000),
    )?;
    within(
        at,
        "idle_timeout_minutes",
        &model.idle_timeout_minutes,
        &(0..=1_440),
    )?;
    within(
        at,
        "start_wait_seconds",
        &model.start_wait_seconds,
        &(10..=3_600),
    )?;
    within(
        at,
        "request_hold_seconds",
        &model
            .request_hold_seconds
            .unwrap_or(model.start_wait_seconds),
        &(0..=model.start_wait_seconds),
    )
}

/// What reaches `vllm serve`.
fn check_vllm(
    at: &str,
    model: &ModelDocument,
    profiles: &[(String, ProfileDocument)],
) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    for (profile_at, profile) in profiles {
        within(
            profile_at,
            "max_model_len",
            &profile.max_model_len,
            &(1_024..=model.context_window),
        )?;
        within(
            profile_at,
            "max_num_seqs",
            &profile.max_num_seqs,
            &(1..=1_024),
        )?;
        // A range check that is false for NaN as well.
        within(profile_at, "gpu_util", &profile.gpu_util, &(0.1..=1.0))?;
    }
    bounded_text(&key("hf_model"), &model.hf_model, 200)?;
    if let Some(effort) = &model.reasoning_effort {
        ensure(
            REASONING_EFFORTS.contains(&effort.as_str()),
            &key("reasoning_effort"),
            "must be one of low, medium, high, xhigh",
        )?;
    }
    if let Some(sampling) = &model.sampling {
        let object = serde_json::from_str::<serde_json::Value>(sampling)
            .is_ok_and(|parsed| parsed.is_object());
        ensure(object, &key("sampling"), "must be a JSON object")?;
    }
    for (profile_at, profile) in profiles {
        let key = format!("{profile_at}.extra_vllm_args");
        ensure(
            profile.extra_vllm_args.len() <= 64,
            &key,
            "must hold at most 64 entries",
        )?;
        for argument in &profile.extra_vllm_args {
            bounded_text(&key, argument, 256)?;
        }
    }
    Ok(())
}

/// Where a pod is placed and what it mounts.
fn check_placement(
    at: &str,
    model: &ModelDocument,
    profiles: &[(String, ProfileDocument)],
) -> Result<(), Refusal> {
    let key = |name: &str| format!("{at}.{name}");
    within(at, "disk_gb", &model.disk_gb, &(10..=2_000))?;
    bounded_text(&key("image"), &model.image, 200)?;
    for (profile_at, profile) in profiles {
        let gpu_key = format!("{profile_at}.gpu_types");
        ensure(
            (1..=16).contains(&profile.gpu_types.len()),
            &gpu_key,
            "must name 1..=16 candidates",
        )?;
        for gpu in &profile.gpu_types {
            bounded_text(&gpu_key, gpu, 64)?;
        }
        ensure(
            (1..=32).contains(&profile.name.len())
                && profile
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
            &format!("{profile_at}.name"),
            "must be 1..=32 of A-Z, a-z, 0-9, - and _",
        )?;
    }
    if let Some(volume) = &model.network_volume_id {
        ensure(
            (1..=64).contains(&volume.len())
                && volume.bytes().all(|byte| byte.is_ascii_alphanumeric()),
            &key("network_volume_id"),
            "must be 1..=64 ASCII alphanumeric characters",
        )?;
        ensure(
            !model.data_center_ids.is_empty(),
            &key("network_volume_id"),
            "requires data_center_ids: the volume exists in one data center",
        )?;
    }
    let mount = &model.volume_mount_path;
    ensure(
        mount.starts_with('/')
            && !mount.ends_with('/')
            && mount.len() <= 128
            && mount
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_')),
        &key("volume_mount_path"),
        "must be an absolute path of at most 128 characters without a trailing slash",
    )?;
    ensure(
        model.data_center_ids.len() <= 16,
        &key("data_center_ids"),
        "must name at most 16 data centers",
    )?;
    for data_center in &model.data_center_ids {
        ensure(
            (2..=32).contains(&data_center.len())
                && data_center
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'),
            &key("data_center_ids"),
            "entries must be 2..=32 of A-Z, 0-9 and -",
        )?;
    }
    Ok(())
}

/// The checked document, mapped onto the settings llm-gateway already has.
fn into_model(model: ModelDocument, profiles: Vec<(String, ProfileDocument)>) -> Model {
    let cache = model.network_volume_id.map(|volume_id| NetworkVolume {
        volume_id,
        mount_path: model.volume_mount_path,
    });
    let thinking = match model.thinking {
        ThinkingDocument::On => Thinking::On,
        ThinkingDocument::Off => Thinking::Off,
    };
    let vllm = |profile: &ProfileDocument| VllmSettings {
        max_model_len: profile.max_model_len,
        max_num_seqs: profile.max_num_seqs,
        gpu_memory_utilization: profile.gpu_util,
        thinking,
        reasoning_effort: model.reasoning_effort.clone(),
        sampling: model.sampling.clone(),
        extra_args: profile.extra_vllm_args.clone(),
    };
    let mut profiles = profiles.into_iter().map(|(_, profile)| profile);
    // `declared_profiles` holds at least one.
    let first = profiles.next().unwrap_or_else(|| ProfileDocument {
        name: "default".to_owned(),
        gpu_types: Vec::new(),
        max_model_len: 0,
        max_num_seqs: default_max_num_seqs(),
        gpu_util: default_gpu_util(),
        extra_vllm_args: Vec::new(),
    });
    let first_vllm = vllm(&first);
    let fallbacks = profiles
        .map(|profile| RunpodProfile {
            vllm: vllm(&profile),
            name: profile.name,
            gpu_types: profile.gpu_types,
        })
        .collect();
    Model {
        provider: model.provider,
        wires: model.wires,
        context_window: model.context_window,
        hf_model: model.hf_model,
        image: model.image,
        gpu_types: first.gpu_types,
        disk_gb: model.disk_gb,
        vllm: first_vllm,
        profile: first.name,
        fallbacks,
        cache,
        data_center_ids: model.data_center_ids,
        idle_timeout_minutes: model.idle_timeout_minutes,
        start_wait_seconds: model.start_wait_seconds,
        request_hold_seconds: model
            .request_hold_seconds
            .unwrap_or(model.start_wait_seconds),
        vllm_api_key_file: model.vllm_api_key_file,
        tool_calling: match model.tool_calling {
            ToolCallingDocument::Parsed => ToolCalling::Parsed,
            ToolCallingDocument::Absent => ToolCalling::Absent,
        },
        price_per_hour_micro_usd: None,
    }
}
