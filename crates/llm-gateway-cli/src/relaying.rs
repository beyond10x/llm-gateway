//! Relaying model calls to Runpod pods (row B8): the gateway's `RelayTargets` over one
//! `RunpodPool`, and every input that pool is built from.
//!
//! The specification is `spec/domains/deployment.yaml` (the `DECIDED` notes on the relay and on
//! `idle_timeout_minutes = 0`) and the `Relay` command of `spec/domains/gateway.yaml`. Each
//! model's pod is asked for through the pool; the gateway sends it the model's vLLM key as
//! `authorization: Bearer <key>`, and nothing of the client's request head.
//!
//! The pool needs a Runpod transport and a way to open a connection to a pod. The shipped
//! binary ([`crate::start`]) composes it over the production transport, `ConnectorsRunpod`, for
//! the provider that declares a `connectors` connection, and reaches each pod over verified TLS
//! ([`crate::ProxyConnector`]); [`crate::start_relaying`] takes any transport, and only tests
//! call it, with `EmulatedRunpod` and loopback pods.

use crate::{
    config::{Deployment, Model, Provider, Wire, may_start_on_request},
    hosted::{HostedEndpoint, HostedTargets, hosted_endpoints},
    keys::VllmKeys,
    pricing::TokenPrice,
    refusal::{Refusal, StartupRefusal},
    serve::{Running, Stopped, inventory, owner_verifier},
    translate::LlmTranslator,
};
use llm_gateway::{
    Gateway, Label, Metrics, OwnerToken, OwnerVerifier, Relay, RelayModel, RelayStream,
    RelayTarget, RelayTargets, RouteInventory, ShutdownReport, TargetBearer, TargetRefusal,
    TargetScheme,
};
use llm_runpod::{
    CleanupReport, Clock, ComputeAuthorization, ConnectorsBinding, ConnectorsRunpod, Hold,
    HostingPolicy, Identifier, LeaseRegistry, ModelPhase, ModelStatus, PodListing, PodStream,
    PodTls, PoolError, RestBinding, RestRunpod, RunpodModel, RunpodPool, RunpodTransport,
    SpendCeiling, SpendPolicy, SpendRefusal, StreamLease,
};
use std::{
    collections::BTreeMap,
    io,
    net::SocketAddr,
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The hosting controller's own identity, written onto every pod it creates.
pub const CONTROLLER: &str = "b10x-llm-gateway";
/// The provider every model's pod runs on.
pub const PROVIDER: &str = "runpod";
/// The Runpod account. The gateway holds no Runpod credential (row K7), so it names none.
pub const ACCOUNT: &str = "default";
/// The budget ledger the hosting contract attaches each pod to.
pub const LEDGER: &str = "b10x-llm-gateway";
/// The reservation every pod is created under when no provider declares a spend ceiling: no
/// ledger is consulted.
pub const RESERVATION: &str = "unmetered";
/// The reservation every pod is created under when a provider declares a spend ceiling: each
/// create is admitted by the pool's spend check.
pub const METERED_RESERVATION: &str = "spend-ceiling";
/// The longest a pod lives before it owes a stop: 24 hours.
pub const MAX_LIFETIME_MS: u64 = 86_400_000;
/// How long one ownership lease lasts: 5 minutes, five cleanup intervals.
pub const LEASE_MS: u64 = 300_000;
/// Container restarts that make a pod crash-looping, as llmgw.
pub const CRASH_RESTART_LIMIT: u32 = 2;
/// The window those restarts are counted in: 10 minutes.
pub const CRASH_WINDOW_MS: u64 = 600_000;
/// How often the running gateway steps the pool's cleanup pass, as llmgw's reaper (row L13).
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);
/// How long a request waiting for its pod waits between two asks of the pool.
const POLL: Duration = Duration::from_millis(500);
/// How often a waiting request looks for the stop between two asks.
const STOP_CHECK: Duration = Duration::from_millis(20);

/// Opens connections to pods. The production one, [`crate::ProxyConnector`], speaks TLS to the
/// Runpod proxy and verifies the pod's certificate before it returns; a test's may reach a
/// loopback pod.
pub trait PodConnector: Send + Sync {
    /// Opens one connection to the pod at `authority`, the `host[:port]` of the endpoint the
    /// pool reported. Its read and write timeouts are the connector's.
    ///
    /// # Errors
    /// The connection's own error; the gateway answers it `upstream-failed`.
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>>;

    /// Opens the readiness probe's connection to the pod at `authority` of an `https://`
    /// endpoint, `timeout` bounding the connect and each read and write. The default opens none,
    /// and the probe answers such an endpoint unreachable.
    ///
    /// # Errors
    /// The connection's own error; the probe answers it unreachable.
    fn connect_probe(
        &self,
        _authority: &str,
        _timeout: Duration,
    ) -> io::Result<Box<dyn RelayStream>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this pod connector opens no probe connection",
        ))
    }
}

/// The transport's readiness probe over a [`PodConnector`]: the TLS connection
/// `ConnectorsRunpod` probes an `https://` endpoint through.
pub(crate) struct ProbeOver(pub(crate) Arc<dyn PodConnector>);

impl PodTls for ProbeOver {
    fn connect(&self, authority: &str, timeout: Duration) -> io::Result<Box<dyn PodStream>> {
        Ok(Box::new(self.0.connect_probe(authority, timeout)?))
    }
}

/// System time in Unix milliseconds that never moves backwards: a step back reads as the last
/// instant read.
#[derive(Debug, Default)]
pub struct WallClock {
    last: AtomicU64,
}

impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        self.last.fetch_max(now, Ordering::SeqCst).max(now)
    }
}

fn identifier(value: &str, key: &str) -> Result<Identifier, Refusal> {
    Identifier::new(value).map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{key} cannot be a hosting identifier: {error}"),
        )
    })
}

/// The hosting policy: fixed values, and one pod per declared model.
///
/// # Errors
/// A `config:value` [`Refusal`] if a fixed value were invalid, or for more models than one
/// controller may hold.
pub fn hosting_policy(deployment: &Deployment) -> Result<HostingPolicy, Refusal> {
    let policy = HostingPolicy {
        controller: identifier(CONTROLLER, "the controller")?,
        provider: identifier(PROVIDER, "the provider")?,
        account: identifier(ACCOUNT, "the account")?,
        ledger: identifier(LEDGER, "the ledger")?,
        max_active: u32::try_from(deployment.models.len()).unwrap_or(u32::MAX),
        max_lifetime_ms: MAX_LIFETIME_MS,
        lease_ms: LEASE_MS,
    };
    // The check the pool's controller makes, made here so that it is made before a transport.
    policy.validate().map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("the hosting policy cannot be run: {error}"),
        )
    })?;
    Ok(policy)
}

/// The unmetered authorization a pod is created under when no spend ceiling is declared.
///
/// # Errors
/// A `config:value` [`Refusal`] if a fixed value were invalid.
pub fn compute_authorization() -> Result<ComputeAuthorization, Refusal> {
    Ok(ComputeAuthorization {
        ledger: identifier(LEDGER, "the ledger")?,
        reservation: identifier(RESERVATION, "the reservation")?,
    })
}

/// The authorization every pod of `deployment` is created under: the reservation
/// [`METERED_RESERVATION`] when [`spend_policy`] binds a spend ceiling (one declared by a
/// provider that one of its models runs on), [`RESERVATION`] otherwise.
///
/// # Errors
/// As [`spend_policy`].
pub fn compute_authorization_for(deployment: &Deployment) -> Result<ComputeAuthorization, Refusal> {
    let reservation = if spend_policy(deployment)?.is_metered() {
        METERED_RESERVATION
    } else {
        RESERVATION
    };
    Ok(ComputeAuthorization {
        ledger: identifier(LEDGER, "the ledger")?,
        reservation: identifier(reservation, "the reservation")?,
    })
}

/// The provider, by name, whose ceilings [`spend_policy`] gives the pool composed from
/// `deployment`: the first, in name order, that one of `deployment`'s models runs on and that
/// declares a daily or a total cap. `None` leaves the pool unmetered.
fn enforced_ceiling(deployment: &Deployment) -> Option<(&String, &Provider)> {
    let billed = |name: &String| {
        deployment
            .models
            .values()
            .any(|model| &model.provider == name)
    };
    deployment
        .providers
        .iter()
        .filter(|(name, _)| billed(name))
        .find(|(_, provider)| {
            provider.daily_cap_micro_usd.is_some() || provider.total_cap_micro_usd.is_some()
        })
}

/// Whether a call may start `model`'s pod in the pool composed from `deployment`, the one place
/// the relay decides it (story:a-call-starts-no-pod): the model may start on request under its
/// own provider's daily cap ([`may_start_on_request`]), and that provider's ceilings are the
/// ones the pool enforces ([`enforced_ceiling`]), so the start is checked against that daily
/// cap. In any other composition, which only a `Deployment` built without the loader reaches,
/// the model behaves as without the key.
fn starts_on_request(deployment: &Deployment, model: &Model) -> bool {
    may_start_on_request(model, &deployment.providers)
        && enforced_ceiling(deployment).is_some_and(|(name, _)| name == &model.provider)
}

/// The spend ledger the pool built from `deployment` checks every create against: the ceilings
/// of the one provider that declares any (the document allows at most one) when one of
/// `deployment`'s models runs on it, and every model's declared price per hour. The pool bills
/// the account of the providers its models name, so a ceiling declared on a provider no model
/// names binds nothing here. With no such ceiling it is unmetered and the pool reads no spend.
///
/// # Errors
/// A `config:value` [`Refusal`] for a model alias that is not an identifier.
pub fn spend_policy(deployment: &Deployment) -> Result<SpendPolicy, Refusal> {
    let mut policy = SpendPolicy::default();
    if let Some((_, provider)) = enforced_ceiling(deployment) {
        policy.daily_cap_micro_usd = provider.daily_cap_micro_usd;
        policy.total_cap_micro_usd = provider.total_cap_micro_usd;
    }
    for (alias, model) in &deployment.models {
        if let Some(price) = model.price_per_hour_micro_usd {
            policy
                .price_per_hour_micro_usd
                .insert(identifier(alias, "a model alias")?, price);
        }
    }
    Ok(policy)
}

/// Where the pool built from `deployment` keeps its spend ledger (story:spend-cap-counts-earlier-pods):
/// `None` when the pool is unmetered, and otherwise the `spend_ledger_file` of the provider
/// whose ceilings bind it, else, when `default` (the shipped compositions), `spend-ledger.json`
/// in the state directory of this process's environment. A test seam passes `default = false`,
/// so a document that names no file keeps no ledger there and no test touches a real one.
///
/// # Errors
/// A `config:value` [`Refusal`] naming `providers.<name>.spend_ledger_file` when the file is
/// the default and neither `XDG_STATE_HOME` nor `HOME` is an absolute path; as
/// [`spend_policy`] otherwise.
fn spend_ledger(
    deployment: &Deployment,
    default: bool,
) -> Result<Option<crate::FileLedger>, Refusal> {
    if !spend_policy(deployment)?.is_metered() {
        return Ok(None);
    }
    let Some((name, provider)) = enforced_ceiling(deployment) else {
        return Ok(None);
    };
    let path = match (&provider.spend_ledger_file, default) {
        (Some(path), _) => path.clone(),
        (None, false) => return Ok(None),
        (None, true) => crate::spend_ledger::default_path(
            std::env::var_os("XDG_STATE_HOME"),
            std::env::var_os("HOME"),
        )
        .ok_or_else(|| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!(
                    "providers.{name}.spend_ledger_file: none declared, and neither \
                     XDG_STATE_HOME nor HOME is set"
                ),
            )
        })?,
    };
    Ok(Some(crate::FileLedger::new(path)))
}

/// `pool` with `ledger` as its spend ledger, read now; unchanged without one.
fn with_ledger<T: RunpodTransport>(
    pool: RunpodPool<T>,
    ledger: Option<crate::FileLedger>,
) -> RunpodPool<T> {
    match ledger {
        Some(ledger) => pool.with_ledger(Box::new(ledger)),
        None => pool,
    }
}

/// `idle_timeout_minutes` as the pool's idle window. 0 is no idle grace (row K27): the pool takes
/// no zero window, so it is 1 ms, and the pod is stopped by the first cleanup pass that finds it
/// quiet, never sooner than its measured cold start.
pub const fn idle_timeout_ms(minutes: u64) -> u64 {
    if minutes == 0 {
        1
    } else {
        minutes.saturating_mul(60_000)
    }
}

/// The Runpod secret the model's pod reads its vLLM key from: `vllm_<alias>`, with every `-`
/// and `.` of the alias written `_`.
pub fn secret_name(alias: &str) -> String {
    let mut name = String::from("vllm_");
    name.extend(alias.chars().map(|character| {
        if matches!(character, '-' | '.') {
            '_'
        } else {
            character
        }
    }));
    name
}

fn runpod_model(
    deployment: &Deployment,
    alias: &str,
    model: &Model,
) -> Result<RunpodModel, Refusal> {
    let at = format!("models.{alias}");
    let cloud_type = deployment
        .providers
        .get(&model.provider)
        .map(|provider| provider.cloud_type)
        .ok_or_else(|| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("{at}.provider must name a declared provider"),
            )
        })?;
    let declared = RunpodModel {
        hf_model: model.hf_model.clone(),
        image: identifier(&model.image, &format!("{at}.image"))?,
        gpu_types: model.gpu_types.clone(),
        cloud_type,
        container_disk_gb: model.disk_gb,
        cache: model.cache.clone(),
        data_center_ids: model.data_center_ids.clone(),
        vllm: model.vllm.clone(),
        api_key_secret: secret_name(alias),
        startup_deadline_ms: model.start_wait_seconds.saturating_mul(1_000),
        idle_timeout_ms: idle_timeout_ms(model.idle_timeout_minutes),
        crash_restart_limit: CRASH_RESTART_LIMIT,
        crash_window_ms: CRASH_WINDOW_MS,
        profile: model.profile.clone(),
        fallbacks: model.fallbacks.clone(),
    };
    declared.validate().map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{at} cannot be run on Runpod: {error}"),
        )
    })?;
    Ok(declared)
}

/// Every declared model as the pool runs it.
///
/// # Errors
/// A `config:value` [`Refusal`] naming the first model that cannot be run.
pub fn runpod_models(
    deployment: &Deployment,
) -> Result<BTreeMap<Identifier, RunpodModel>, Refusal> {
    let mut models = BTreeMap::new();
    for (alias, model) in &deployment.models {
        models.insert(
            identifier(alias, &format!("models.{alias}"))?,
            runpod_model(deployment, alias, model)?,
        );
    }
    Ok(models)
}

/// The pool the deployment describes, over `transport`, on `clock`, with a fresh lease registry.
///
/// # Errors
/// A `config:value` [`Refusal`] when a model or the policy cannot be run.
pub fn runpod_pool<T: RunpodTransport>(
    deployment: &Deployment,
    transport: T,
    clock: Arc<dyn Clock>,
) -> Result<RunpodPool<T>, Refusal> {
    let spend = spend_policy(deployment)?;
    RunpodPool::new(
        hosting_policy(deployment)?,
        Arc::new(LeaseRegistry::default()),
        transport,
        runpod_models(deployment)?,
        clock,
    )
    .map(|pool| {
        let pool = pool.with_spend(spend);
        match pool_provider(deployment) {
            Some(provider) => pool.with_proxy_url(provider.proxy_url.clone()),
            None => pool,
        }
    })
    .map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("the Runpod pool cannot be composed: {error}"),
        )
    })
}

/// The provider whose pods the pool of `deployment` runs: the first model's.
fn pool_provider(deployment: &Deployment) -> Option<&crate::config::Provider> {
    let model = deployment.models.values().next()?;
    deployment.providers.get(&model.provider)
}

/// What one relayed model needs at acquisition.
#[derive(Clone)]
struct Relayed {
    alias: Identifier,
    bearer: Arc<TargetBearer>,
    /// How long a request waits for the model's pod: `request_hold_seconds` (row K28).
    hold: Duration,
    /// Whether a request may start the model's pod: its `start_on_request`, under a daily cap
    /// on its provider ([`starts_on_request`]). Without it a request only uses or waits on a
    /// pod the owner's wake started (story:a-call-starts-no-pod).
    start_on_request: bool,
    /// Each profile's `max_model_len` by profile name: the window of a pod started on it.
    windows: BTreeMap<String, u64>,
}

/// What each model of `deployment` needs at acquisition: its alias, its vLLM key as the bearer,
/// its hold, and its profiles' windows.
///
/// # Errors
/// A `config:value` [`Refusal`] for a model that names no `vllm_api_key_file`: its pod always
/// expects a key; `vllm-api-key:not-a-token` for a key a header cannot carry.
fn relayed_models(
    deployment: &Deployment,
    keys: &VllmKeys,
) -> Result<BTreeMap<String, Relayed>, Refusal> {
    let mut models = BTreeMap::new();
    for (alias, model) in &deployment.models {
        let at = format!("models.{alias}.vllm_api_key_file");
        let key = keys.get(alias).ok_or_else(|| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("{at} is required to relay to a Runpod pod, or its vllm_api_key"),
            )
        })?;
        let bearer = TargetBearer::new(key.expose().to_vec()).map_err(|error| {
            Refusal::new(
                StartupRefusal::VllmApiKeyNotAToken,
                format!("{at}: {error}"),
            )
        })?;
        models.insert(
            alias.clone(),
            Relayed {
                alias: identifier(alias, &format!("models.{alias}"))?,
                bearer: Arc::new(bearer),
                hold: Duration::from_secs(model.request_hold_seconds),
                // The cap rule holds where a start is decided, not only in the loader: a
                // `Deployment` built without it gets no request start.
                start_on_request: starts_on_request(deployment, model),
                windows: profile_windows(model).collect(),
            },
        );
    }
    // An alias acquires, wakes and reads its model's pod: the same pool entry under a second
    // name, so it starts no pod of its own.
    for (alias, declared) in &deployment.aliases {
        if let Some(relayed) = models.get(&declared.model).cloned() {
            models.insert(alias.clone(), relayed);
        }
    }
    Ok(models)
}

/// Each profile of `model` with its `max_model_len`, the first profile first.
fn profile_windows(model: &Model) -> impl Iterator<Item = (String, u64)> + '_ {
    std::iter::once((model.profile.clone(), u64::from(model.vllm.max_model_len))).chain(
        model.fallbacks.iter().map(|fallback| {
            (
                fallback.name.clone(),
                u64::from(fallback.vllm.max_model_len),
            )
        }),
    )
}

/// The gateway's targets: the pod the pool hands out for each alias.
pub(crate) struct PoolTargets<T> {
    pool: Arc<RunpodPool<T>>,
    authorization: ComputeAuthorization,
    models: BTreeMap<String, Relayed>,
    connector: Arc<dyn PodConnector>,
    /// Set when the gateway starts to stop: a request still waiting for its pod gives up.
    stopping: Arc<AtomicBool>,
    /// The pod counters (row O1), shared with the relay that serves them.
    metrics: Arc<Metrics>,
    /// Per alias, the last deployment counted as a pod start.
    started: Mutex<BTreeMap<String, Identifier>>,
    /// The provider whose spend ceilings the pool enforces ([`enforced_ceiling`]); `None` when
    /// it enforces none (story:limits-read).
    ceilings: Option<String>,
}

impl<T> PoolTargets<T> {
    /// # Errors
    /// A `config:value` [`Refusal`] for a model that names no `vllm_api_key_file`: its pod
    /// always expects a key.
    pub(crate) fn new(
        deployment: &Deployment,
        keys: &VllmKeys,
        pool: Arc<RunpodPool<T>>,
        connector: Arc<dyn PodConnector>,
        stopping: Arc<AtomicBool>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, Refusal> {
        Ok(Self::from_relayed(
            relayed_models(deployment, keys)?,
            compute_authorization_for(deployment)?,
            pool,
            connector,
            stopping,
            metrics,
        )
        .with_ceilings(deployment))
    }

    /// The targets naming the provider whose ceilings the pool built from `deployment`
    /// enforces, the same [`enforced_ceiling`] [`spend_policy`] gives the pool.
    fn with_ceilings(mut self, deployment: &Deployment) -> Self {
        self.ceilings = enforced_ceiling(deployment).map(|(name, _)| name.clone());
        self
    }

    fn from_relayed(
        models: BTreeMap<String, Relayed>,
        authorization: ComputeAuthorization,
        pool: Arc<RunpodPool<T>>,
        connector: Arc<dyn PodConnector>,
        stopping: Arc<AtomicBool>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            pool,
            authorization,
            models,
            connector,
            stopping,
            metrics,
            started: Mutex::new(BTreeMap::new()),
            ceilings: None,
        }
    }

    /// Counts the pod a hold bound to as started, once per deployment (row O1). The pool binds
    /// a hold on the `starting` answer of the very ask that submitted the create, and names each
    /// deployment it creates anew, so a deployment not yet counted for its alias is a create the
    /// pool submitted. A request that binds to a pod another request started counts nothing.
    fn count_start(&self, alias: &str, hold: &Hold) {
        if let Some(deployment) = hold.deployment() {
            self.count_started(alias, deployment);
        }
    }

    /// Counts `deployment`, found starting for `alias`, as a pod start unless it already was.
    /// A wake that submitted a create finds its own deployment starting; one that found a pod
    /// a request or an earlier wake started finds it counted.
    fn count_started(&self, alias: &str, deployment: &Identifier) {
        let mut started = self.started.lock().unwrap_or_else(PoisonError::into_inner);
        if started.get(alias) != Some(deployment) {
            started.insert(alias.to_owned(), deployment.clone());
            drop(started);
            self.metrics.count_pod_start();
            tracing::info!(
                model = alias,
                deployment = deployment.as_str(),
                "pod start submitted"
            );
        }
    }

    /// Counts a pod start the spend check refused, and writes the ceiling and the excess,
    /// which the client's refusal never names, into a `warn` event.
    fn count_spend_refusal(&self, alias: &str, refusal: SpendRefusal) {
        self.metrics.count_pod_start_spend_refusal();
        match refusal {
            SpendRefusal::Over { over_micro_usd, .. } => tracing::warn!(
                model = alias,
                ceiling = refusal.code(),
                over_usd = llm_runpod::usd(over_micro_usd).as_str(),
                "pod start refused by a spend ceiling"
            ),
            SpendRefusal::Unknown => tracing::warn!(
                model = alias,
                ceiling = refusal.code(),
                "pod start refused by a spend ceiling"
            ),
        }
    }

    /// Counts a pool answer that says a pod could not be started or failed while starting
    /// (row O1). The pool gives a failure's reason to exactly one ask, so each is counted once.
    fn count_failure(&self, alias: &str, error: PoolError) {
        if matches!(
            error,
            PoolError::NoCapacity
                | PoolError::CrashLoop
                | PoolError::CredentialRefused
                | PoolError::StartupDeadline
        ) {
            self.metrics.count_pod_start_failure();
            tracing::warn!(model = alias, reason = error.code(), "pod start failed");
        }
    }
}

/// Counts what one cleanup pass stopped (row O1): idle, retired and replaced deployments and
/// orphaned pods. A refused pass stopped nothing it can name.
fn count_reaps(metrics: &Metrics, pass: Result<CleanupReport, PoolError>) {
    let Ok(report) = pass else {
        return;
    };
    let stopped =
        report.idle.len() + report.retired.len() + report.replaced.len() + report.orphans.len();
    if stopped > 0 {
        metrics.count_pod_reaps(u64::try_from(stopped).unwrap_or(u64::MAX));
        tracing::info!(
            idle = report.idle.len(),
            retired = report.retired.len(),
            replaced = report.replaced.len(),
            orphans = report.orphans.len(),
            "cleanup pass stopped pods"
        );
    }
}

/// The `host[:port]` of an endpoint URL such as `https://<pod>-8000.proxy.runpod.net/v1/`.
fn authority(endpoint: &str) -> Option<&str> {
    let rest = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))?;
    let authority = rest.split('/').next()?;
    (!authority.is_empty()).then_some(authority)
}

/// The scheme of an endpoint URL: `https` for the Runpod proxy's `https://<pod>-8000.proxy.runpod.net/v1/`,
/// `http` for an `http://` endpoint (an emulated pod). A translated request reaches the pod over
/// it (story:gateway-translation); the byte relay always connects through the pod connector.
fn scheme(endpoint: &str) -> TargetScheme {
    if endpoint.starts_with("http://") {
        TargetScheme::Http
    } else {
        TargetScheme::Https
    }
}

impl<T: RunpodTransport + Send + 'static> RelayTargets for PoolTargets<T> {
    /// Asks the pool for the model's ready pod through one [`Hold`], and while the pool answers
    /// `starting` (or `stopping`, before the hold is bound to a pod) holds the request, asking
    /// again every 500 ms, until the model's `request_hold_seconds` have passed (row L6). Each
    /// ask is one pool step under the pool's lock and the wait is outside it. The first ask may
    /// start a pod; the hold then binds to the pod it found starting, so requests held together
    /// start one pod. A bound pod still starting when the hold passes is `model-cold-start`
    /// (row W6); a hold that passes still unbound, waiting out a previous pod's unconfirmed
    /// stop, is `target-unavailable`. Once the bound pod is retired, marked stop-required or
    /// gone from the slot, whichever step retired it, the hold is lost: the request is
    /// `target-unavailable` at once and starts no replacement. Every other pool refusal is
    /// `target-unavailable` too. Once the gateway starts to stop, a request may
    /// still use a pod that is ready now, but starts none and waits for none: without a ready
    /// pod it gives up at once (`target-unavailable`), so it cannot hold the graceful stop.
    ///
    /// Only a model with `start_on_request` under a daily cap asks with the compute
    /// authorization ([`starts_on_request`]), so only its
    /// request may start a pod. Any other asks `RunpodPool::ensure_held_running`, which never
    /// submits a create (story:a-call-starts-no-pod): a pod the owner's wake started is held on
    /// and relayed to as above, but with no pod running or starting (the model stopped, or its
    /// pod stopping) the request is `model-stopped` at once, without waiting out its hold.
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        let relayed = self.models.get(alias).ok_or(TargetRefusal::Unavailable)?;
        let deadline = Instant::now() + relayed.hold;
        let mut hold = Hold::default();
        loop {
            let stopping = self.stopping.load(Ordering::SeqCst);
            let asked = if stopping {
                self.pool.ensure_running(&relayed.alias)
            } else if relayed.start_on_request {
                self.pool
                    .ensure_held(&relayed.alias, &self.authorization, &mut hold)
            } else {
                self.pool.ensure_held_running(&relayed.alias, &mut hold)
            };
            self.count_start(relayed.alias.as_str(), &hold);
            if let Err(error) = &asked {
                self.count_failure(alias, *error);
            }
            match asked {
                Ok(lease) => {
                    let endpoint = lease.endpoint().ok_or(TargetRefusal::Unavailable)?;
                    let authority = authority(endpoint)
                        .ok_or(TargetRefusal::Unavailable)?
                        .to_owned();
                    let scheme = scheme(endpoint);
                    // The window of the profile the pod was started on; a pod whose profile is
                    // unknown leaves the model's, the smallest of its profiles'.
                    let max_model_len = lease
                        .profile()
                        .and_then(|profile| relayed.windows.get(profile).copied());
                    return Ok(Box::new(PodTarget {
                        authority,
                        scheme,
                        max_model_len,
                        bearer: Arc::clone(&relayed.bearer),
                        connector: Arc::clone(&self.connector),
                        // A hold binds only to a pod it found starting (rows O1, O2).
                        held: hold.deployment().is_some(),
                        lease,
                    }));
                }
                Err(PoolError::Starting | PoolError::Stopping) if !stopping && !hold.lost() => {
                    // An unbound hold found no pod starting: without `start_on_request` none
                    // will start for this request, so it is not held.
                    if !relayed.start_on_request && hold.deployment().is_none() {
                        return Err(TargetRefusal::Stopped);
                    }
                    if Instant::now() >= deadline {
                        // Only a request bound to a pod still starting is told to come back;
                        // one still waiting out a previous pod's unconfirmed stop had no pod
                        // starting for it (row W6).
                        return Err(if hold.deployment().is_some() {
                            TargetRefusal::ColdStart
                        } else {
                            TargetRefusal::Unavailable
                        });
                    }
                    let next = (Instant::now() + POLL).min(deadline);
                    while Instant::now() < next && !self.stopping.load(Ordering::SeqCst) {
                        thread::sleep(
                            STOP_CHECK.min(next.saturating_duration_since(Instant::now())),
                        );
                    }
                }
                Err(PoolError::SpendCeiling(refusal)) => {
                    self.count_spend_refusal(alias, refusal);
                    return Err(TargetRefusal::SpendCeiling);
                }
                Err(_) => return Err(TargetRefusal::Unavailable),
            }
        }
    }

    /// A request through the pod at `authority` failed (row W7): the pool stops that pod if it
    /// is still the model's current one. For a model with `start_on_request` the next request
    /// starts a replacement; for any other the next request is `model-stopped` until the owner
    /// wakes the model. A report about a pod already replaced changes nothing.
    fn invalidate(&self, alias: &str, failed: &str) {
        if let Some(relayed) = self.models.get(alias) {
            let _stopped = self.pool.invalidate(&relayed.alias, |endpoint| {
                authority(endpoint) == Some(failed)
            });
        }
    }

    /// The owner's wake (story:model-wake-and-status): one `RunpodPool::wake`, the first ask
    /// of a held request, so the spend check and the profile fallback are a request's, and
    /// its pod start, spend refusal or start failure is counted as a request's would be. A
    /// spend refusal is `spend-ceiling`; every other pool refusal `target-unavailable`. Once
    /// the gateway starts to stop it starts nothing and steps nothing: a model starting or
    /// ready is answered its status, any other `target-unavailable`. A model this pool does
    /// not run (one of a provider with no `connectors` connection) is `target-unavailable`, as
    /// its requests are.
    fn wake(&self, alias: &str) -> Option<Result<llm_gateway::ModelStatus, TargetRefusal>> {
        let Some(relayed) = self.models.get(alias) else {
            return Some(Err(TargetRefusal::Unavailable));
        };
        if self.stopping.load(Ordering::SeqCst) {
            return Some(match self.pool.status(&relayed.alias) {
                Ok(status) if matches!(status.phase, ModelPhase::Starting | ModelPhase::Ready) => {
                    Ok(gateway_status(&status))
                }
                _ => Err(TargetRefusal::Unavailable),
            });
        }
        Some(match self.pool.wake(&relayed.alias, &self.authorization) {
            Ok(status) => {
                if status.phase == ModelPhase::Starting
                    && let Some(deployment) = &status.deployment
                {
                    self.count_started(relayed.alias.as_str(), deployment);
                }
                Ok(gateway_status(&status))
            }
            Err(PoolError::SpendCeiling(refusal)) => {
                self.count_spend_refusal(alias, refusal);
                Err(TargetRefusal::SpendCeiling)
            }
            Err(error) => {
                self.count_failure(alias, error);
                Err(TargetRefusal::Unavailable)
            }
        })
    }

    /// `RunpodPool::status`: the phase the pool's records hold, with no provider call. A model
    /// this pool does not run (one of a provider with no `connectors` connection) is `stopped`,
    /// as [`Unconnected`] answers it.
    fn status(&self, alias: &str) -> Option<llm_gateway::ModelStatus> {
        let stopped = || llm_gateway::ModelStatus::new(llm_gateway::ModelPhase::Stopped);
        let Some(relayed) = self.models.get(alias) else {
            return Some(stopped());
        };
        Some(
            self.pool
                .status(&relayed.alias)
                .map_or_else(|_| stopped(), |status| gateway_status(&status)),
        )
    }

    /// `RunpodPool::spend_ceilings` (story:limits-read): each ceiling as the pool's spend check
    /// counts it now, with one spend read under the pool's lock, named by the provider that
    /// declares it. Empty, with no read, when the pool enforces none.
    fn spend_limits(&self) -> Vec<llm_gateway::SpendLimit> {
        let Some(provider) = &self.ceilings else {
            return Vec::new();
        };
        self.pool
            .spend_ceilings()
            .into_iter()
            .map(|reading| llm_gateway::SpendLimit {
                provider: provider.clone(),
                ceiling: match reading.ceiling {
                    SpendCeiling::Daily => llm_gateway::LimitCeiling::Daily,
                    SpendCeiling::Total => llm_gateway::LimitCeiling::Total,
                },
                cap_micro_usd: reading.cap_micro_usd,
                window_start_ms: reading.window_start_ms,
                window_end_ms: reading.window_end_ms,
                spent_micro_usd: reading.spent_micro_usd,
                last_refusal: reading.last_refusal.map(|(at_ms, model)| {
                    llm_gateway::LimitRefusal {
                        at_ms,
                        model: model.as_str().to_owned(),
                    }
                }),
            })
            .collect()
    }
}

/// The pool's status as the gateway answers it.
fn gateway_status(status: &ModelStatus) -> llm_gateway::ModelStatus {
    llm_gateway::ModelStatus {
        phase: match status.phase {
            ModelPhase::Stopped => llm_gateway::ModelPhase::Stopped,
            ModelPhase::Starting => llm_gateway::ModelPhase::Starting,
            ModelPhase::Ready => llm_gateway::ModelPhase::Ready,
            ModelPhase::Stopping => llm_gateway::ModelPhase::Stopping,
        },
        sub_phase: status.sub_phase.map(str::to_owned),
        profile: status.profile.clone(),
    }
}

/// One pod, held through its [`StreamLease`] until the relayed answer has ended.
struct PodTarget {
    authority: String,
    scheme: TargetScheme,
    bearer: Arc<TargetBearer>,
    connector: Arc<dyn PodConnector>,
    /// The request was held while this pod started.
    held: bool,
    /// The `max_model_len` of the profile the pod was started on, when it is known.
    max_model_len: Option<u64>,
    lease: StreamLease,
}

impl RelayTarget for PodTarget {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn scheme(&self) -> TargetScheme {
        self.scheme
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        Some(&self.bearer)
    }

    fn profile(&self) -> Option<String> {
        self.lease.profile().map(str::to_owned)
    }

    fn max_model_len(&self) -> Option<u64> {
        self.max_model_len
    }

    /// Every pod is reached through Runpod's proxy, which closes a connection silent past its
    /// bound: every call is sent streamed (story:runpod-rest-host).
    fn streamed(&self) -> bool {
        true
    }

    fn held(&self) -> bool {
        self.held
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        self.connector.connect(&self.authority)
    }
}

/// The pool's cleanup pass, run every `interval` ([`CLEANUP_INTERVAL`] when relaying, row L13)
/// on its own thread until stopped.
struct Cleanup {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: JoinHandle<()>,
}

impl Cleanup {
    fn start(interval: Duration, pass: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let stopped = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            let (lock, wake) = &*stopped;
            let mut stopping = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                let (next, _) = wake
                    .wait_timeout_while(stopping, interval, |stop| !*stop)
                    .unwrap_or_else(PoisonError::into_inner);
                if *next {
                    return;
                }
                drop(next);
                pass();
                stopping = lock.lock().unwrap_or_else(PoisonError::into_inner);
            }
        });
        Self { stop, thread }
    }

    fn stop(self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        drop(self.thread.join());
    }
}

/// A gateway that relays model calls to Runpod pods, and the pool's cleanup pass beside it.
///
/// Stopping it stops no pod: a pod created during the run keeps running, and billing, until its
/// idle limit would have passed and the next run's startup sweep terminates it.
pub struct Relaying {
    running: Running,
    /// The address the metrics-only listener bound; `None` without `metrics_listen`.
    metrics_addr: Option<SocketAddr>,
    /// The pool's cleanup pass; `None` when no pool is composed.
    cleanup: Option<Cleanup>,
    stopping: Arc<AtomicBool>,
    /// The interaction records' writer the stop waits on; `None` without `[interaction_records]`.
    records: Option<crate::records::Drain>,
}

impl Relaying {
    pub fn local_addr(&self) -> SocketAddr {
        self.running.local_addr()
    }

    /// The address the metrics-only listener bound, the port the system picked when
    /// `metrics_listen` names port 0; `None` when the document sets no `metrics_listen`.
    pub fn metrics_addr(&self) -> Option<SocketAddr> {
        self.metrics_addr
    }

    /// The dashboard's state, for the account panel and the heartbeat's route activity.
    pub fn dashboard(&self) -> Arc<llm_gateway::Dashboard> {
        self.running.dashboard()
    }

    /// Drains and stops the gateway gracefully now, then stops the cleanup pass. A request
    /// still waiting for its pod is answered `target-unavailable` at once. With
    /// `[interaction_records]` it then waits at most 2 s for the writer to store its queue and
    /// names the calls still queued on one line (D96).
    pub fn shutdown(self) -> ShutdownReport {
        self.stopping.store(true, Ordering::SeqCst);
        let report = self.running.shutdown();
        if let Some(cleanup) = self.cleanup {
            cleanup.stop();
        }
        if let Some(records) = self.records {
            let left = records.finish();
            if left > 0 {
                crate::serve::report(format_args!(
                    "interaction records: {left} calls not stored at stop"
                ));
            }
        }
        report
    }

    /// Blocks until SIGINT or SIGTERM, then stops as [`Self::shutdown`] does.
    pub fn wait_for_stop(mut self) -> Stopped {
        let signal = self.running.wait_for_signal();
        Stopped {
            signal,
            report: self.shutdown(),
        }
    }
}

fn relay_label(value: &str) -> Result<Label, Refusal> {
    Label::new(value).map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("{value:?} cannot be relayed as a model name: {error}"),
        )
    })
}

pub(crate) const fn relay_wire(wire: Wire) -> llm_gateway::Wire {
    match wire {
        Wire::Chat => llm_gateway::Wire::Chat,
        Wire::Responses => llm_gateway::Wire::Responses,
        Wire::Messages => llm_gateway::Wire::Messages,
    }
}

/// Composes the gateway like [`crate::start`], and also relays each model on its wires to the
/// pod a `RunpodPool` over `transport` hands out, sending the model's vLLM key as the bearer
/// (row B8). Every model must name a `vllm_api_key_file`. The pool runs one cleanup pass before
/// the gateway is marked ready and then one every [`CLEANUP_INTERVAL`] until the stop.
///
/// The shipped binary never calls this: it composes [`start_connected`]. This is the seam a
/// test composes with `EmulatedRunpod` and loopback pods.
///
/// # Errors
/// As [`crate::start`], plus `config:value` for a model the pool cannot run or that names no
/// `vllm_api_key_file`.
pub fn start_relaying<T: RunpodTransport + Send + 'static>(
    deployment: &Deployment,
    transport: T,
    pods: Arc<dyn PodConnector>,
    clock: Arc<dyn Clock>,
) -> Result<Relaying, Refusal> {
    let parts = Parts::read(deployment, &pods)?;
    let ledger = spend_ledger(deployment, false)?;
    let pool = Arc::new(with_ledger(
        runpod_pool(deployment, transport, clock)?,
        ledger,
    ));
    let targets = PoolTargets::new(
        deployment,
        &parts.vllm_keys,
        Arc::clone(&pool),
        pods,
        Arc::clone(&parts.stopping),
        Arc::clone(&parts.metrics),
    )?;
    let pass = move |metrics: &Metrics| count_reaps(metrics, pool.reap());
    compose(deployment, parts, Arc::new(targets), Some(Box::new(pass)))
}

/// The binary's own pod target source for `deployment` over `transport`, with nothing else of
/// the gateway composed: one `RunpodPool` on `clock`, each model's `keys` entry as its vLLM
/// bearer, and every pod reached through `pods`. No file the document names is read and no
/// cleanup pass runs. The shipped binary never calls this: it is the seam the conformance
/// runner serves a scenario's models from over `EmulatedRunpod` (story:a-call-starts-no-pod).
///
/// # Errors
/// As [`start_relaying`] for the pool and the keys: `config:value` for a model the pool cannot
/// run or that has no entry in `keys`.
pub fn pod_targets<T: RunpodTransport + Send + 'static>(
    deployment: &Deployment,
    keys: &BTreeMap<String, Vec<u8>>,
    transport: T,
    pods: Arc<dyn PodConnector>,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn RelayTargets>, Refusal> {
    let ledger = spend_ledger(deployment, false)?;
    let pool = Arc::new(with_ledger(
        runpod_pool(deployment, transport, clock)?,
        ledger,
    ));
    let targets = PoolTargets::new(
        deployment,
        &VllmKeys::given(keys),
        pool,
        pods,
        Arc::new(AtomicBool::new(false)),
        Arc::new(Metrics::default()),
    )?;
    Ok(Arc::new(targets))
}

/// Targets for models no pool serves: every acquisition and every wake is
/// `target-unavailable`, and every such model is `stopped`, as [`PoolTargets`] answers a model
/// of a provider it does not run. Only a Runpod model reaches it: a
/// hosted model is answered before it ([`HostedTargets`]). `reason`, when given, is why
/// (story:runpod-rest-host): every refusal names it.
#[derive(Default)]
struct Unconnected {
    reason: Option<String>,
}

impl Unconnected {
    /// The models of `provider`, which was not reachable at start.
    fn unreachable(provider: &str) -> Self {
        Self {
            reason: Some(format!("provider {provider} was not reachable at start")),
        }
    }
}

impl RelayTargets for Unconnected {
    fn unavailable_reason(&self, _alias: &str) -> Option<String> {
        self.reason.clone()
    }

    fn acquire(&self, _alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        Err(TargetRefusal::Unavailable)
    }

    fn invalidate(&self, _alias: &str, _failed: &str) {}

    fn wake(&self, _alias: &str) -> Option<Result<llm_gateway::ModelStatus, TargetRefusal>> {
        Some(Err(TargetRefusal::Unavailable))
    }

    fn status(&self, _alias: &str) -> Option<llm_gateway::ModelStatus> {
        Some(llm_gateway::ModelStatus::new(
            llm_gateway::ModelPhase::Stopped,
        ))
    }
}

/// Whether the work directory is an existing directory of the effective user with no group or
/// world permission.
fn private_directory(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_dir()
            // No group or world bit: the low six bits of the mode are clear.
            && metadata.mode().trailing_zeros() >= 6
            && metadata.uid() == rustix::process::geteuid().as_raw()
    })
}

/// The reachability check (`llm-gateway.deployment`, DECIDED): a private work directory and one
/// complete `pods.list` through the transport.
fn reachable(binding: &ConnectorsBinding, transport: &mut ConnectorsRunpod) -> bool {
    private_directory(&binding.work_directory)
        && transport
            .list_pods()
            .is_ok_and(|listing: PodListing| listing.complete)
}

/// The composition the shipped binary runs ([`crate::start`]): every model is relayed; the
/// models of the provider that declares a `connectors` connection through one `RunpodPool`
/// over `wrap(ConnectorsRunpod)`, every other model `target-unavailable`. Each connected
/// model's vLLM key is handed to the transport by alias for the readiness probe. When the
/// connection is not reachable at start, no pool is composed, no further `connectors` call is
/// made, and its models are `target-unavailable` too.
///
/// `pods` opens every connection to a pod: the relay's, and the readiness probe's to an
/// `https://` endpoint ([`PodConnector::connect_probe`], handed to the transport as its TLS
/// connection before `wrap`). `wrap` and `pods` are the seam a test moves the pod's address to
/// a loopback pod with. `wrap` receives the `ConnectorsRunpod` built from the document and may
/// return any `RunpodTransport`, including one that ignores it; the type does not hold the
/// transport to `ConnectorsRunpod`. What does is the callers: [`crate::start`], the only
/// composition the shipped binary reaches, passes the identity and
/// [`crate::ProxyConnector::shipped`], and only tests pass anything else. No document key and no
/// option reaches `wrap` or `pods`.
///
/// # Errors
/// As [`start_relaying`], for the connected models.
pub fn start_connected<W, F>(
    deployment: &Deployment,
    wrap: F,
    pods: Arc<dyn PodConnector>,
) -> Result<Relaying, Refusal>
where
    W: RunpodTransport + Send + 'static,
    F: FnOnce(ConnectorsRunpod) -> W,
{
    let parts = Parts::read(deployment, &pods)?;
    if let Some((provider, key)) = crate::account::keyed_provider(deployment) {
        return start_rest(deployment, parts, provider, key, pods);
    }
    let connected = deployment
        .providers
        .iter()
        .find_map(|(name, provider)| Some((name, provider.connectors.clone()?)));
    let Some((provider, binding)) = connected else {
        return compose(deployment, parts, Arc::new(Unconnected::default()), None);
    };
    // The pool runs this provider's models on this provider's account: the subset holds no
    // other provider, so no other provider's spend ceiling binds it.
    let mut subset = deployment.clone();
    subset.models.retain(|_, model| &model.provider == provider);
    subset.providers.retain(|name, _| name == provider);
    if subset.models.is_empty() {
        return compose(deployment, parts, Arc::new(Unconnected::default()), None);
    }
    // Every check the pool and its targets make, made before the transport is built: whether
    // a document is valid never depends on the connection, and a refused document costs no
    // `connectors` call.
    runpod_models(&subset)?;
    hosting_policy(&subset)?;
    spend_policy(&subset)?;
    let ledger = spend_ledger(&subset, true)?;
    let authorization = compute_authorization_for(&subset)?;
    let relayed = relayed_models(&subset, &parts.vllm_keys)?;
    let mut transport = ConnectorsRunpod::new(binding.clone());
    transport.set_tls(Arc::new(ProbeOver(Arc::clone(&pods))));
    if !reachable(&binding, &mut transport) {
        tracing::warn!(
            provider = provider.as_str(),
            "connectors connection unreachable; its models answer target-unavailable"
        );
        return compose(deployment, parts, Arc::new(Unconnected::default()), None);
    }
    for alias in subset.models.keys() {
        if let Some(key) = parts.vllm_keys.get(alias) {
            transport.set_vllm_key(alias, key.expose().to_vec());
        }
    }
    // The one constructor every composition builds its pool with, so none starts a pod
    // without the spend ceilings the document declares.
    let pool = Arc::new(with_ledger(
        runpod_pool(&subset, wrap(transport), Arc::new(WallClock::default()))?,
        ledger,
    ));
    let targets = PoolTargets::from_relayed(
        relayed,
        authorization,
        Arc::clone(&pool),
        pods,
        Arc::clone(&parts.stopping),
        Arc::clone(&parts.metrics),
    )
    .with_ceilings(&subset);
    let pass = move |metrics: &Metrics| count_reaps(metrics, pool.reap());
    compose(deployment, parts, Arc::new(targets), Some(Box::new(pass)))
}

/// The composition for a provider that names its Runpod API key by a keychain entry
/// (story:keychain-references) or a file (story:runpod-key-file): the key is read once, from the
/// OS keychain or through the trusted-file reader, before any request is sent, and the
/// provider's models run through one `RunpodPool` over `RestRunpod`, which calls Runpod's REST
/// API itself over the pod connector's TLS. When one complete pod listing cannot be read at
/// start, no pool is composed and its models are `target-unavailable`, as for an unreachable
/// `connectors` connection. A key that cannot be read refuses the start with an
/// `endpoint-credential:*` refusal naming `providers.<name>.api_key` or
/// `providers.<name>.api_key_file`.
fn start_rest(
    deployment: &Deployment,
    parts: Parts,
    provider: &str,
    where_key: crate::account::ProviderKey<'_>,
    pods: Arc<dyn PodConnector>,
) -> Result<Relaying, Refusal> {
    let mut subset = deployment.clone();
    subset.models.retain(|_, model| model.provider == provider);
    subset.providers.retain(|name, _| name == provider);
    if subset.models.is_empty() {
        return compose(deployment, parts, Arc::new(Unconnected::default()), None);
    }
    runpod_models(&subset)?;
    hosting_policy(&subset)?;
    spend_policy(&subset)?;
    let ledger = spend_ledger(&subset, true)?;
    let authorization = compute_authorization_for(&subset)?;
    let relayed = relayed_models(&subset, &parts.vllm_keys)?;
    let key = crate::account::read_key(provider, where_key)?;
    let binding = rest_binding(&subset.providers[provider]);
    let mut transport = RestRunpod::new(binding.clone(), key);
    transport.set_tls(Arc::new(ProbeOver(Arc::clone(&pods))));
    if !transport
        .list_pods()
        .is_ok_and(|listing: PodListing| listing.complete)
    {
        let (host, cause) = failure_fields(transport.last_failure(), &binding);
        tracing::warn!(
            provider,
            host = host.as_str(),
            cause = cause.as_str(),
            "Runpod REST API unreachable; its models answer target-unavailable"
        );
        return compose(
            deployment,
            parts,
            Arc::new(Unconnected::unreachable(provider)),
            None,
        );
    }
    for alias in subset.models.keys() {
        if let Some(key) = parts.vllm_keys.get(alias) {
            transport.set_vllm_key(alias, key.expose().to_vec());
        }
    }
    let pool = Arc::new(with_ledger(
        runpod_pool(&subset, transport, Arc::new(WallClock::default()))?,
        ledger,
    ));
    let targets = PoolTargets::from_relayed(
        relayed,
        authorization,
        Arc::clone(&pool),
        pods,
        Arc::clone(&parts.stopping),
        Arc::clone(&parts.metrics),
    )
    .with_ceilings(&subset);
    let pass = move |metrics: &Metrics| count_reaps(metrics, pool.reap());
    compose(deployment, parts, Arc::new(targets), Some(Box::new(pass)))
}

/// Where a provider with an `api_key` manages its pods: its `management_url`, Runpod's GraphQL
/// and the 30 s bound (story:runpod-rest-host).
pub(crate) fn rest_binding(provider: &crate::config::Provider) -> RestBinding {
    RestBinding {
        base_url: provider.management_url.clone(),
        ..RestBinding::runpod()
    }
}

/// The host and the cause a warning about a failed Runpod read names: the last failed call's,
/// or the management API's host and an incomplete listing when no call failed. Never the key.
pub(crate) fn failure_fields(
    failure: Option<llm_runpod::CallFailure>,
    binding: &RestBinding,
) -> (String, String) {
    failure.map_or_else(
        || {
            let host = binding
                .base_url
                .split_once("://")
                .map_or(binding.base_url.as_str(), |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            (host, "the pod listing was not complete".to_owned())
        },
        |failure| (failure.host, failure.cause.to_string()),
    )
}

/// What every composition reads from the document first, in this order, and the state the
/// relay and the stop share.
struct Parts {
    verifier: Arc<dyn OwnerVerifier>,
    inventory: RouteInventory,
    vllm_keys: VllmKeys,
    /// Each hosted model's and each route's endpoint, its credential read
    /// (story:hosted-endpoints, story:routes-document).
    hosted: BTreeMap<String, HostedEndpoint>,
    /// Each Bedrock model, with the container credentials it signs with
    /// (story:bedrock-hosted-target).
    bedrock: BTreeMap<String, Arc<crate::bedrock::BedrockRoute>>,
    /// The connector an `https` hosted endpoint is reached through: the pod connector.
    tls: Arc<dyn PodConnector>,
    stopping: Arc<AtomicBool>,
    metrics: Arc<Metrics>,
    /// Every model as the relay serves it.
    models: Vec<RelayModel>,
    /// The metrics listener's scrape token, when `metrics_bearer_file` names one.
    metrics_bearer: Option<OwnerToken>,
    /// The caller review of a document with `ingress_auth = "service-account-token"`.
    reviewer: Option<crate::token_review::TokenReviewer>,
}

impl Parts {
    /// # Errors
    /// An `owner-secret:*`, `vllm-api-key:*`, `endpoint-credential:*` or `config:value`
    /// [`Refusal`]: every refusal the document itself can earn, before any transport is built.
    fn read(deployment: &Deployment, tls: &Arc<dyn PodConnector>) -> Result<Self, Refusal> {
        Ok(Self {
            verifier: owner_verifier(deployment)?,
            metrics_bearer: crate::serve::metrics_bearer(deployment)?,
            reviewer: crate::token_review::token_reviewer(deployment)?,
            vllm_keys: crate::keys::vllm_keys(deployment)?,
            hosted: hosted_endpoints(deployment)?,
            bedrock: crate::bedrock::routes(deployment)?,
            tls: Arc::clone(tls),
            inventory: inventory(deployment)?,
            stopping: Arc::new(AtomicBool::new(false)),
            metrics: Arc::new(Metrics::default()),
            models: relay_models(deployment)?,
        })
    }
}

/// Every model of the document as the relay serves it: on its wires, under its alias, with its
/// `tool_calling`, and with the `max_model_len` and `context_window` the public routes publish
/// (rows R5, R1). A Runpod model's `max_model_len` is the smallest of its profiles': the window
/// every pod it may be served by takes; the output clamp takes the window of the profile the
/// serving pod was started on instead ([`PodTarget`]). A hosted model is relayed under its
/// `upstream_model`, and publishes the `context_window` it declares and no `max_model_len`.
/// Only a hosted model gives a messages request naming no effort its declared
/// `reasoning_effort`: a Runpod model's is already its server's default chat-template
/// argument. A route of the routes document is relayed as
/// [`crate::Routes::relay_models`] says.
/// A hosted model's output cap, and a client route's beta extras and newest family.
fn route_settings(
    relayed: RelayModel,
    hosted: &crate::config::Hosted,
    alias: &str,
) -> Result<RelayModel, Refusal> {
    let refused = |key: &str, error: llm_gateway::RelayError| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("hosted.{alias}.{key}: {error}"),
        )
    };
    let relayed = match hosted.max_tokens_cap {
        Some(cap) => relayed.with_max_tokens_cap(u64::from(cap)),
        None => relayed,
    };
    let relayed = if hosted.anthropic_beta.is_empty() {
        relayed
    } else {
        relayed
            .with_anthropic_beta(hosted.anthropic_beta.clone())
            .map_err(|error| refused("anthropic_beta", error))?
    };
    let relayed = match hosted.compact_window {
        Some(window) => relayed.with_compact_window(u64::from(window)),
        None => relayed,
    };
    let relayed = match hosted.retry {
        (None, None) => relayed,
        (attempts, wait) => relayed.with_retry(
            attempts.unwrap_or(llm_gateway::DEFAULT_RETRY_ATTEMPTS),
            wait.map_or(llm_gateway::DEFAULT_RETRY_WAIT, |seconds| {
                std::time::Duration::from_secs(u64::from(seconds))
            }),
        ),
    };
    match &hosted.newest {
        Some(family) => relayed
            .with_newest(family)
            .map_err(|error| refused("newest", error)),
        None => Ok(relayed),
    }
}

fn relay_models(deployment: &Deployment) -> Result<Vec<RelayModel>, Refusal> {
    let mut models = Vec::with_capacity(deployment.models.len() + deployment.hosted.len());
    for (alias, hosted) in &deployment.hosted {
        let wires = hosted.wires.iter().copied().map(relay_wire).collect();
        let relayed = RelayModel::new(
            relay_label(alias)?,
            relay_label(&hosted.upstream_model)?,
            wires,
        )
        .map_err(|error| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("hosted.{alias}: {error}"),
            )
        })?
        .with_tool_calling(hosted.tool_calling)
        .with_kind(match hosted.kind {
            crate::config::HostedKind::Endpoint => llm_gateway::ModelKind::Hosted,
            crate::config::HostedKind::Ollama => llm_gateway::ModelKind::Ollama,
        });
        let relayed = match hosted.reasoning_effort {
            Some(effort) => relayed.with_reasoning_effort(effort),
            None => relayed,
        };
        let relayed = if hosted.auth_kind == llm_gateway::AuthKind::Client {
            relayed.with_client_credentials()
        } else {
            relayed
        };
        let relayed = route_settings(relayed, hosted, alias)?;
        let relayed = match &hosted.model_prefix {
            Some(prefix) => relayed.with_model_prefix(prefix).map_err(|error| {
                Refusal::new(
                    StartupRefusal::ConfigValue,
                    format!("hosted.{alias}.model_prefix: {error}"),
                )
            })?,
            None => relayed,
        };
        let relayed = priced(relayed, hosted.token_price.as_ref());
        models.push(match hosted.context_window {
            Some(window) => relayed.with_context_window(u64::from(window)),
            None => relayed,
        });
    }
    // A Bedrock model takes no client wire verbatim: the translator sends every request to it
    // with llm's Bedrock client, under the alias (story:bedrock-hosted-target).
    for (alias, model) in &deployment.bedrock {
        let relayed = RelayModel::new(
            relay_label(alias)?,
            relay_label(alias)?,
            vec![llm_gateway::Wire::Messages],
        )
        .map_err(|error| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("hosted.{alias}: {error}"),
            )
        })?
        .with_translated_only()
        .with_tool_calling(model.tool_calling)
        .with_kind(llm_gateway::ModelKind::Bedrock);
        let relayed = priced(relayed, model.token_price.as_ref());
        models.push(match model.context_window {
            Some(window) => relayed.with_context_window(u64::from(window)),
            None => relayed,
        });
    }
    for (alias, model) in &deployment.models {
        models.push(runpod_relay_model(deployment, alias, alias, model)?);
    }
    // An alias is its model under a second name, sent as that model to that model's pod, with
    // its forced effort.
    for (alias, declared) in &deployment.aliases {
        let Some(model) = deployment.models.get(&declared.model) else {
            return Err(Refusal::new(
                StartupRefusal::ConfigValue,
                format!("aliases.{alias}.model must name a [models.*] model"),
            ));
        };
        models.push(
            runpod_relay_model(deployment, alias, &declared.model, model)?
                .with_forced_effort(declared.effort),
        );
    }
    // Each route of the routes document, on its endpoint's wire (story:routes-document).
    models.extend(deployment.routes.relay_models()?);
    Ok(models)
}

/// The Runpod model `model` relayed as `alias`, sent to its pod as `served`.
fn runpod_relay_model(
    deployment: &Deployment,
    alias: &str,
    served: &str,
    model: &Model,
) -> Result<RelayModel, Refusal> {
    let wires = model.wires.iter().copied().map(relay_wire).collect();
    // The pod serves the model under its alias (`--served-model-name`), so the upstream name is
    // the model's alias.
    let relayed = RelayModel::new(relay_label(alias)?, relay_label(served)?, wires)
        .map_err(|error| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("models.{served}: {error}"),
            )
        })?
        .with_tool_calling(model.tool_calling)
        .with_max_model_len(
            profile_windows(model)
                .map(|(_, window)| window)
                .min()
                .unwrap_or(u64::from(model.vllm.max_model_len)),
        )
        .with_context_window(u64::from(model.context_window));
    let relayed = priced(relayed, model.token_price.as_ref());
    // The status list says what `acquire` does, from the same decision
    // (story:a-call-starts-no-pod).
    Ok(if starts_on_request(deployment, model) {
        relayed
    } else {
        relayed.with_no_call_start()
    })
}

/// `relayed`, priced by its declared token price (story:cost-per-call).
fn priced(relayed: RelayModel, price: Option<&TokenPrice>) -> RelayModel {
    match price {
        Some(price) => relayed.with_pricing(price.pricing()),
        None => relayed,
    }
}

/// One cleanup pass of a pool, counting what it stopped.
type CleanupPass = Box<dyn Fn(&Metrics) + Send>;

/// The gateway relaying every model of `deployment` to `targets`, with `pass` run once before
/// it is marked ready and then every [`CLEANUP_INTERVAL`].
fn compose(
    deployment: &Deployment,
    parts: Parts,
    targets: Arc<dyn RelayTargets>,
    pass: Option<CleanupPass>,
) -> Result<Relaying, Refusal> {
    let Parts {
        verifier,
        inventory,
        vllm_keys,
        hosted,
        bedrock,
        tls,
        stopping,
        metrics,
        models,
        metrics_bearer,
        reviewer,
    } = parts;
    // Every composition serves its hosted models from their endpoints, whatever serves the rest.
    let targets: Arc<dyn RelayTargets> = Arc::new(HostedTargets::new(hosted, tls, targets));
    let targets: Arc<dyn RelayTargets> =
        Arc::new(crate::bedrock::BedrockTargets::new(deployment, targets));
    // Every composition translates a wire a model's upstream does not speak
    // (story:gateway-translation), a route under its declared capabilities
    // (story:routes-document).
    let translator = bedrock
        .into_iter()
        .fold(LlmTranslator::new(), |translator, (alias, route)| {
            translator.with_bedrock(alias, route)
        });
    let translator = deployment.routes.translator(translator);
    let relay = Relay::new(models, targets)
        .map_err(|error| Refusal::new(StartupRefusal::ConfigValue, format!("models: {error}")))?
        .with_translator(Arc::new(translator))
        .with_metrics(Arc::clone(&metrics))
        .with_decoder(Arc::new(crate::decode::Decoder))
        .with_assembler(Arc::new(crate::assemble::EventAssembler))
        .with_records(Arc::new(crate::logging::TracingRecords));
    // Every finished call into the session store, when the document declares one
    // (story:interaction-record-store).
    let (relay, records) =
        crate::records::compose(relay, deployment.interaction_records.as_ref(), &metrics)?;
    let relay = match reviewer {
        Some(reviewer) => relay.with_caller_review(Arc::new(reviewer)),
        None => relay,
    };
    let signals = Running::install_signals()?;
    let bind = deployment.gateway.bind;
    let mut handle =
        Gateway::bind_with_relay(deployment.gateway.clone(), verifier, inventory, relay).map_err(
            |error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")),
        )?;
    let metrics_addr =
        match deployment.metrics_listen {
            Some(scrape) => Some(handle.serve_metrics(scrape, metrics_bearer).map_err(
                |error| {
                    Refusal::new(
                        StartupRefusal::ListenBind,
                        format!("metrics_listen {scrape}: {error}"),
                    )
                },
            )?),
            None => None,
        };
    // One pass before serving: it sweeps the pods a previous run of this controller left. A
    // refused pass changes nothing, and the next one runs on time.
    let cleanup = pass.map(|pass| {
        pass(&metrics);
        Cleanup::start(CLEANUP_INTERVAL, move || pass(&metrics))
    });
    handle.mark_ready();
    Ok(Relaying {
        running: Running::from_parts(handle, signals, vllm_keys),
        metrics_addr,
        cleanup,
        stopping,
        records,
    })
}

#[cfg(test)]
#[path = "relaying_stop_tests.rs"]
mod stop_tests;

#[cfg(test)]
mod tests {
    use super::{Cleanup, authority, idle_timeout_ms, secret_name};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    /// Row L13 on the timer itself, at a 50 ms interval instead of 60 s: no pass before the
    /// first interval, one per interval after it, and none once stopped. The stop does not wait
    /// for the next interval.
    #[test]
    fn l13_the_cleanup_pass_runs_on_its_interval_until_it_is_stopped() {
        let passes = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&passes);
        let cleanup = Cleanup::start(Duration::from_millis(50), move || {
            counting.fetch_add(1, Ordering::SeqCst);
        });
        thread::sleep(Duration::from_millis(20));
        assert_eq!(
            passes.load(Ordering::SeqCst),
            0,
            "a pass ran before its interval"
        );
        thread::sleep(Duration::from_millis(400));
        let ran = passes.load(Ordering::SeqCst);
        assert!(ran >= 2, "{ran} passes in 420 ms at a 50 ms interval");
        let stopping = Instant::now();
        cleanup.stop();
        assert!(
            stopping.elapsed() < Duration::from_millis(45),
            "the stop waited {:?} for the next interval",
            stopping.elapsed()
        );
        let stopped = passes.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(150));
        assert_eq!(
            passes.load(Ordering::SeqCst),
            stopped,
            "a pass ran after the stop"
        );
    }

    #[test]
    fn the_authority_is_the_endpoints_host_and_port() {
        assert_eq!(
            authority("https://pod1-8000.proxy.runpod.net/v1/"),
            Some("pod1-8000.proxy.runpod.net")
        );
        assert_eq!(authority("http://127.0.0.1:9/v1/"), Some("127.0.0.1:9"));
        for refused in ["", "pod1", "https:///v1/", "ftp://pod/"] {
            assert_eq!(authority(refused), None, "{refused:?}");
        }
    }

    /// A Runpod proxy endpoint is reached over `https` by a translated request; an emulated
    /// pod's `http://` endpoint over `http` (story:gateway-translation).
    #[test]
    fn translation_a_pod_target_reports_its_endpoints_scheme() {
        use llm_gateway::TargetScheme;
        assert_eq!(
            super::scheme("https://pod1-8000.proxy.runpod.net/v1/"),
            TargetScheme::Https
        );
        assert_eq!(super::scheme("http://127.0.0.1:9/v1/"), TargetScheme::Http);
    }

    #[test]
    fn the_secret_name_is_the_alias_in_runpods_secret_alphabet() {
        assert_eq!(secret_name("small"), "vllm_small");
        assert_eq!(secret_name("qwen-3.8_b"), "vllm_qwen_3_8_b");
    }

    #[test]
    fn an_idle_timeout_of_minutes_is_milliseconds_and_zero_is_the_smallest_window() {
        assert_eq!(idle_timeout_ms(0), 1);
        assert_eq!(idle_timeout_ms(1), 60_000);
        assert_eq!(idle_timeout_ms(30), 1_800_000);
        assert_eq!(idle_timeout_ms(1_440), 86_400_000);
    }

    /// story:route-status-for-every-model: each relayed model names what serves it, which the
    /// status list renders as its `kind`.
    #[test]
    fn route_status_each_relayed_model_names_its_kind() {
        use llm_gateway::ModelKind;
        let deployment = crate::parse(
            "listen = \"127.0.0.1:0\"\ningress_auth = \"none\"\n\n[providers.runpod]\nkind = \"runpod-vllm\"\n\n\
             [models.small]\nprovider = \"runpod\"\nwires = [\"chat\"]\ncontext_window = 65536\n\
             hf_model = \"example/small-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\n\
             gpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n\n\
             [hosted.remote]\nwires = [\"chat\"]\nbase_url = \"http://127.0.0.1:9/v1\"\n\
             upstream_model = \"vendor-remote\"\nauth_kind = \"anonymous\"\n\n\
             [hosted.claude]\nkind = \"bedrock\"\nregion = \"eu-central-1\"\n\
             model_id = \"anthropic.claude-test-v1:0\"\ncredential_source = \"container\"\n\n\
             [hosted.local]\nkind = \"ollama\"\nupstream_model = \"qwen2.5:1.5b\"\n",
        )
        .unwrap();
        let kinds: Vec<(String, ModelKind)> = super::relay_models(&deployment)
            .unwrap()
            .iter()
            .map(|model| (model.alias().as_str().to_owned(), model.kind()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("local".to_owned(), ModelKind::Ollama),
                ("remote".to_owned(), ModelKind::Hosted),
                ("claude".to_owned(), ModelKind::Bedrock),
                ("small".to_owned(), ModelKind::Pod),
            ]
        );
    }
}
