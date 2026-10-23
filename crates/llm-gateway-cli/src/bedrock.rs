//! A model Amazon Bedrock serves (story:bedrock-hosted-target): `[hosted.<alias>]` of
//! `kind = "bedrock"`, sent with llm's Bedrock client and signed with the credentials the
//! container credentials endpoint answers.
//!
//! The endpoint is `AWS_CONTAINER_CREDENTIALS_FULL_URI` and the token it is asked with is read
//! from `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`, through the trusted-file reader at the start and
//! again for every fetch, so a rotated token is picked up. Both are read only when the document
//! declares a Bedrock model. Nothing here contacts the endpoint at the start: the first turn
//! does. No region, model id, endpoint, credentials URI or token is ever rendered by the route
//! inventory or written to the log.

use crate::{
    config::{Bedrock, Deployment, hex_digest},
    refusal::{Refusal, StartupRefusal},
    trusted,
};
use llm_bedrock::{Api, BedrockTarget, ContainerCredentials, CredentialSource};
use llm_core::{BoxFuture, Capabilities, Error, Id, Protocol, Provenance};
use llm_credentials::{
    ResolvedSecret, Secret, SecretError, SecretRef, SecretResolver, SecretVersion,
};
use llm_gateway::{
    AuthKind, Label, ModelStatus, RelayStream, RelayTarget, RelayTargets, RouteSummary,
    TargetLimits, TargetProvenance, TargetRefusal, TargetSummary, ToolCalling,
};
use llm_http::{HttpClient, Limits};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use url::Url;

/// The variable naming the container credentials endpoint.
pub const CREDENTIALS_URI_VARIABLE: &str = "AWS_CONTAINER_CREDENTIALS_FULL_URI";
/// The variable naming the file of the token the endpoint is asked with.
pub const TOKEN_FILE_VARIABLE: &str = "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE";
/// The most a container authorization token holds once one line end is removed: llm-bedrock's
/// own bound.
pub const MAX_CONTAINER_TOKEN_BYTES: usize = 16 * 1024;
/// How long the credentials endpoint may take to accept a connection.
const CREDENTIALS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long it may take to answer, and between two reads of its answer.
const CREDENTIALS_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one fetch may take in all.
const CREDENTIALS_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// The context window, and output limit, a Bedrock model that declares none is translated under.
pub(crate) const DEFAULT_CONTEXT_WINDOW: u32 = 200_000;
/// The authority a Bedrock model's relay target names. It is never connected to: the translator
/// reaches Bedrock with its own client.
const AUTHORITY: &str = "bedrock";
/// The name the token is resolved under; only [`TokenFile`] answers it.
const TOKEN_REFERENCE: &str = "container-authorization-token";

/// Whether `model_id` names an Anthropic model: a model id (`anthropic.…`) or an inference
/// profile (`eu.anthropic.…`), alone or as the last segment of an ARN. llm's Bedrock client
/// applies the same rule to `thinking` on Converse.
pub(crate) fn anthropic_model(model_id: &str) -> bool {
    let name = model_id.rsplit('/').next().unwrap_or(model_id);
    name.split('.').take(2).any(|part| part == "anthropic")
}

/// The operation a model id is sent to when the document names none: `anthropic-invoke` for an
/// Anthropic model, `converse` otherwise.
pub(crate) fn default_api(model_id: &str) -> Api {
    if anthropic_model(model_id) {
        Api::AnthropicInvoke
    } else {
        Api::Converse
    }
}

const fn protocol(api: Api) -> Protocol {
    match api {
        Api::Converse => Protocol::BedrockConverse,
        Api::AnthropicInvoke => Protocol::Messages,
    }
}

/// The wire label the inventory names a Bedrock target's protocol with.
const fn protocol_label(api: Api) -> &'static str {
    match api {
        Api::Converse => "bedrock-converse",
        Api::AnthropicInvoke => "messages",
    }
}

fn id(value: &str) -> Result<Id, Error> {
    Id::new(value).map_err(|_| Error::invalid("an identifier is not printable ASCII"))
}

/// The binding identity `alias` is served under: the provider `bedrock`, the account
/// `container`, and a revision that is a digest of where it points, so a repointed model is a
/// different binding and no value is shown.
fn provenance(alias: &str, model: &Bedrock) -> Result<Provenance, Error> {
    let api = match model.api {
        Api::Converse => "converse",
        Api::AnthropicInvoke => "anthropic-invoke",
    };
    let endpoint = model.endpoint.as_ref().map_or("", Url::as_str);
    let revision =
        hex_digest(format!("{}\n{}\n{api}\n{endpoint}", model.region, model.model_id).as_bytes());
    Ok(Provenance {
        protocol: protocol(model.api),
        provider: id("bedrock")?,
        account: id("container")?,
        endpoint: id(alias)?,
        model: id(alias)?,
        binding_revision: id(&revision)?,
    })
}

/// What a Bedrock model is translated under: tools when it parses tool calls, temperature and
/// `top_p`, no reasoning effort (it is dropped), its window or [`DEFAULT_CONTEXT_WINDOW`], its
/// declared output limit or else the window, and streamed output.
pub(crate) fn capabilities(
    window: Option<u32>,
    max_output: Option<u32>,
    tool_calling: ToolCalling,
) -> Capabilities {
    let window = u64::from(window.unwrap_or(DEFAULT_CONTEXT_WINDOW));
    let tools = tool_calling == ToolCalling::Parsed;
    Capabilities {
        tools,
        tool_choice: tools,
        temperature: true,
        top_p: true,
        reasoning_efforts: Vec::new(),
        context_window: window,
        max_output_tokens: max_output.map_or(window, u64::from),
        output_streaming: true,
        input_during_output: false,
    }
}

/// A target holding every value llm admits but the one under test, so each key's refusal names
/// that key alone.
fn probe(region: &str, model_id: &str, endpoint: Option<Url>) -> bool {
    let Ok(provenance) = (|| {
        Ok::<_, Error>(Provenance {
            protocol: Protocol::Messages,
            provider: id("bedrock")?,
            account: id("container")?,
            endpoint: id("probe")?,
            model: id("probe")?,
            binding_revision: id("probe")?,
        })
    })() else {
        return false;
    };
    BedrockTarget::new(
        provenance,
        capabilities(None, None, ToolCalling::Parsed),
        region,
        model_id,
        Api::AnthropicInvoke,
        true,
        endpoint,
    )
    .is_ok()
}

/// Whether llm's Bedrock client takes `region`.
pub(crate) fn region_admitted(region: &str) -> bool {
    probe(region, "probe", None)
}

/// Whether llm's Bedrock client takes `model_id`.
pub(crate) fn model_id_admitted(model_id: &str) -> bool {
    probe("eu-central-1", model_id, None)
}

/// `endpoint` as the URL llm's Bedrock client takes, or `None`.
pub(crate) fn endpoint_admitted(endpoint: &str) -> Option<Url> {
    let url = Url::parse(endpoint).ok()?;
    probe("eu-central-1", "probe", Some(url.clone())).then_some(url)
}

/// One Bedrock model as the translator sends to it: the validated target, the credentials it
/// signs with, and the `anthropic-beta` names the deployment allows it to carry, empty for none.
pub(crate) struct BedrockRoute {
    pub(crate) target: BedrockTarget,
    pub(crate) credentials: Arc<dyn CredentialSource>,
    pub(crate) anthropic_beta: Vec<String>,
}

impl BedrockRoute {
    pub(crate) const fn api(&self) -> Api {
        self.target.api
    }

    pub(crate) fn anthropic(&self) -> bool {
        anthropic_model(&self.target.model_id)
    }
}

/// The container authorization token, read from its file through the trusted-file reader for
/// every fetch. A file that has become unreadable or untrusted is `unavailable`, which the
/// Bedrock client answers `unauthorized`, before anything is sent; the refusal's rule code and
/// the file's path are logged as a warning, the token never.
struct TokenFile {
    path: PathBuf,
}

/// Runs `work`, which blocks on the file system, on the runtime's blocking pool when there is a
/// runtime, so a slow mount holds no worker thread; inline otherwise. `None` when it panicked.
async fn off_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => runtime.spawn_blocking(work).await.ok(),
        Err(_) => Some(work()),
    }
}

impl SecretResolver for TokenFile {
    fn resolve<'a>(
        &'a self,
        _reference: &'a SecretRef,
    ) -> BoxFuture<'a, Result<ResolvedSecret, SecretError>> {
        Box::pin(async move {
            let path = self.path.clone();
            let read = off_runtime(move || trusted::read(&path, &trusted::CONTAINER_TOKEN))
                .await
                .ok_or(SecretError::Unavailable)?;
            let text = read.map_err(|refusal| {
                tracing::warn!(
                    rule = refusal.code(),
                    file = refusal.message(),
                    "bedrock: the container token file was refused; the fetch is unauthorized"
                );
                SecretError::Unavailable
            })?;
            let version = SecretVersion::new(hex_digest(text.as_bytes()))?;
            let secret = Secret::new(text.into_bytes())?;
            Ok(ResolvedSecret { secret, version })
        })
    }
}

/// The token's bytes once one trailing line end is removed, as llm-bedrock removes it.
fn without_line_end(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

/// Reads the token file once through the trusted-file reader, so a start with a file llm's
/// client would refuse is refused before anything is served. The bytes are overwritten after.
fn check_token(path: &Path) -> Result<(), Refusal> {
    let mut read = trusted::read(path, &trusted::CONTAINER_TOKEN)
        .map_err(|refusal| {
            Refusal::new(
                refusal.kind(),
                format!("{TOKEN_FILE_VARIABLE} {}", refusal.message()),
            )
        })?
        .into_bytes();
    let token = without_line_end(&read);
    let refuse = |kind, rule: &str| {
        Refusal::new(
            kind,
            format!("{TOKEN_FILE_VARIABLE} {}: {rule}", path.display()),
        )
    };
    let held = if token.len() > MAX_CONTAINER_TOKEN_BYTES {
        Err(refuse(
            StartupRefusal::ContainerTokenTooLarge,
            &format!("the token exceeds {MAX_CONTAINER_TOKEN_BYTES} bytes"),
        ))
    } else if token.is_empty()
        || !token
            .iter()
            .all(|byte| *byte == b' ' || byte.is_ascii_graphic())
    {
        Err(refuse(
            StartupRefusal::ContainerTokenNotAToken,
            "the token must be one line of printable ASCII, not empty",
        ))
    } else {
        Ok(())
    };
    read.fill(0);
    std::hint::black_box(&read);
    held
}

/// A variable's value, `None` when it is unset or empty.
fn variable(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The container credentials the document's Bedrock models sign with, from the environment.
fn container_credentials() -> Result<Arc<dyn CredentialSource>, Refusal> {
    let unset = || {
        Refusal::new(
            StartupRefusal::ContainerCredentialsUnset,
            format!(
                "a Bedrock model needs {CREDENTIALS_URI_VARIABLE} and {TOKEN_FILE_VARIABLE}, \
                 the container credentials endpoint and its token file"
            ),
        )
    };
    let uri_refused = || {
        Refusal::new(
            StartupRefusal::ContainerCredentialsUri,
            format!(
                "{CREDENTIALS_URI_VARIABLE} must be https, or http to a loopback address or \
                 to 169.254.170.2, 169.254.170.23 or [fd00:ec2::23], without user information \
                 or a fragment"
            ),
        )
    };
    let uri = variable(CREDENTIALS_URI_VARIABLE).ok_or_else(unset)?;
    let token = variable(TOKEN_FILE_VARIABLE).ok_or_else(unset)?;
    let uri = uri.into_string().map_err(|_| uri_refused())?;
    let path = PathBuf::from(token);
    let limits = Limits {
        response_headers: CREDENTIALS_READ_TIMEOUT,
        idle: CREDENTIALS_READ_TIMEOUT,
        total: CREDENTIALS_TOTAL_TIMEOUT,
    };
    let http =
        HttpClient::with_connect_timeout(limits, CREDENTIALS_CONNECT_TIMEOUT).map_err(|_| {
            Refusal::new(
                StartupRefusal::ContainerCredentialsUri,
                "the container credentials client could not start",
            )
        })?;
    let reference = SecretRef::new(TOKEN_REFERENCE).map_err(|_| uri_refused())?;
    let credentials = ContainerCredentials::new(
        http,
        &uri,
        Arc::new(TokenFile { path: path.clone() }),
        reference,
    )
    .map_err(|_| uri_refused())?;
    check_token(&path)?;
    Ok(Arc::new(credentials))
}

/// Every Bedrock model of `deployment`, by alias, with the container credentials they share.
/// The environment is read only when there is one.
///
/// # Errors
/// `container-credentials:unset` when either variable is unset or empty,
/// `container-credentials:uri` for a URI llm's client does not admit (never quoted), a
/// `container-token:*` trusted-file rule for the token file, and `config:value` for a model
/// llm's client refuses.
pub(crate) fn routes(
    deployment: &Deployment,
) -> Result<BTreeMap<String, Arc<BedrockRoute>>, Refusal> {
    let mut routes = BTreeMap::new();
    if deployment.bedrock.is_empty() {
        return Ok(routes);
    }
    let credentials = container_credentials()?;
    for (alias, model) in &deployment.bedrock {
        let invalid = || {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("hosted.{alias} cannot be served by llm's Bedrock client"),
            )
        };
        let target = BedrockTarget::new(
            provenance(alias, model).map_err(|_| invalid())?,
            capabilities(
                model.context_window,
                model.max_output_tokens,
                model.tool_calling,
            ),
            model.region.clone(),
            model.model_id.clone(),
            model.api,
            true,
            model.endpoint.clone(),
        )
        .map_err(|_| invalid())?;
        routes.insert(
            alias.clone(),
            Arc::new(BedrockRoute {
                target,
                credentials: Arc::clone(&credentials),
                anthropic_beta: model.anthropic_beta.clone(),
            }),
        );
    }
    Ok(routes)
}

/// The relay target of a Bedrock model: it names [`AUTHORITY`] and is never connected to, since
/// only the translator reaches Bedrock.
struct Unreachable;

impl RelayTarget for Unreachable {
    fn authority(&self) -> &str {
        AUTHORITY
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::other(
            "a Bedrock model is reached only through the translator",
        ))
    }
}

/// The binary's target source with its Bedrock models: one of those is had at once and has no
/// lifecycle; every other alias is `rest`'s.
pub(crate) struct BedrockTargets {
    aliases: Vec<String>,
    rest: Arc<dyn RelayTargets>,
}

impl BedrockTargets {
    pub(crate) fn new(deployment: &Deployment, rest: Arc<dyn RelayTargets>) -> Self {
        Self {
            aliases: deployment.bedrock.keys().cloned().collect(),
            rest,
        }
    }

    fn holds(&self, alias: &str) -> bool {
        self.aliases.iter().any(|held| held == alias)
    }
}

impl RelayTargets for BedrockTargets {
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        if self.holds(alias) {
            return Ok(Box::new(Unreachable));
        }
        self.rest.acquire(alias)
    }

    /// A Bedrock model that failed is kept: the next request asks Bedrock again.
    fn invalidate(&self, alias: &str, authority: &str) {
        if !self.holds(alias) {
            self.rest.invalidate(alias, authority);
        }
    }

    fn wake(&self, alias: &str) -> Option<Result<ModelStatus, TargetRefusal>> {
        if self.holds(alias) {
            return None;
        }
        self.rest.wake(alias)
    }

    fn status(&self, alias: &str) -> Option<ModelStatus> {
        if self.holds(alias) {
            return None;
        }
        self.rest.status(alias)
    }
}

fn label(value: &str) -> Result<Label, Refusal> {
    Label::new(value).map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{value:?} cannot be served as an inventory identifier: {error}"),
        )
    })
}

/// The inventory's route for each Bedrock model: one target `<alias>.bedrock`, provider
/// `bedrock`, account `container`, auth kind `aws-sigv4`. It names the alias and never the
/// region, the model id or the endpoint.
///
/// # Errors
/// A `config:value` [`Refusal`] when a model cannot be represented in an inventory.
pub(crate) fn summaries(deployment: &Deployment) -> Result<Vec<RouteSummary>, Refusal> {
    deployment
        .bedrock
        .iter()
        .map(|(alias, model)| {
            let target = TargetSummary {
                target_id: label(&format!("{alias}.bedrock"))?,
                position: 0,
                provenance: TargetProvenance {
                    protocol: label(protocol_label(model.api))?,
                    provider: label("bedrock")?,
                    account: label("container")?,
                    endpoint: label(alias)?,
                    model: label(alias)?,
                    binding_revision: label(&deployment.digest)?,
                },
                auth_kind: AuthKind::AwsSigv4,
                billing_kind: model.billing_kind,
                limits: TargetLimits {
                    context_window: model.context_window.map(u64::from),
                    max_output_tokens: model.max_output_tokens.map(u64::from),
                },
            };
            RouteSummary::new(label(alias)?, label(alias)?, false, vec![target]).map_err(|error| {
                Refusal::new(StartupRefusal::ConfigValue, format!("inventory: {error}"))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        anthropic_model, endpoint_admitted, model_id_admitted, off_runtime, region_admitted,
    };

    #[test]
    fn the_token_file_is_read_off_the_runtimes_threads() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let caller = std::thread::current().id();
        let reader = runtime
            .block_on(off_runtime(|| std::thread::current().id()))
            .unwrap();
        assert_ne!(reader, caller);
    }

    #[test]
    fn an_anthropic_model_is_named_by_its_first_two_dotted_parts_or_an_arns_last_segment() {
        for id in [
            "anthropic.claude-fixture-v1:0",
            "eu.anthropic.claude-haiku-5-5",
            "arn:aws:bedrock:eu-central-1:000000000000:inference-profile/eu.anthropic.fixture",
        ] {
            assert!(anthropic_model(id), "{id}");
        }
        for id in [
            "eu.amazon.nova-fixture-v1:0",
            "meta.llama-fixture",
            "arn:aws:bedrock:eu-central-1:000000000000:application-inference-profile/fixture",
            "a.b.anthropic",
        ] {
            assert!(!anthropic_model(id), "{id}");
        }
    }

    #[test]
    fn the_region_model_id_and_endpoint_are_held_to_llms_rules() {
        assert!(region_admitted("eu-central-1"));
        assert!(!region_admitted("EU_CENTRAL_1"));
        assert!(!region_admitted("eu"));
        assert!(model_id_admitted("eu.anthropic.claude-haiku-5-5"));
        assert!(!model_id_admitted(""));
        assert!(!model_id_admitted(".."));
        assert!(endpoint_admitted("http://127.0.0.1:9").is_some());
        assert!(endpoint_admitted("https://bedrock.example.test").is_some());
        assert!(endpoint_admitted("http://bedrock.example.test").is_none());
        assert!(endpoint_admitted("https://bedrock.example.test/v1").is_none());
        assert!(endpoint_admitted("not a url").is_none());
    }
}
