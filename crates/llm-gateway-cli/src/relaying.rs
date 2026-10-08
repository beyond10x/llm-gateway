//! Relaying model calls to Runpod pods (row B8): the gateway's `RelayTargets` over one
//! `RunpodPool`, and every input that pool is built from.
//!
//! The specification is `spec/domains/deployment.yaml` (the `DECIDED` notes on the relay and on
//! `idle_timeout_minutes = 0`) and the `Relay` command of `spec/domains/gateway.yaml`. Each
//! model's pod is asked for through the pool; the gateway sends it the model's vLLM key as
//! `authorization: Bearer <key>`, and nothing of the client's request head.
//!
//! The pool needs a Runpod transport and a way to open a connection to a pod. The shipped
//! binary has neither (story:live-runpod-wiring), so only [`crate::start_relaying`] composes
//! this, and only tests call it, with `EmulatedRunpod` and loopback pods.

use crate::{
    config::{Deployment, Model},
    keys::VllmKeys,
    refusal::{Refusal, StartupRefusal},
};
use llm_gateway::{RelayStream, RelayTarget, RelayTargets, TargetBearer};
use llm_runpod::{
    Clock, ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry, PoolError, RunpodModel,
    RunpodPool, RunpodTransport, StreamLease,
};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The hosting controller's own identity, written onto every pod it creates.
pub const CONTROLLER: &str = "b10x-llm-gateway";
/// The provider every model's pod runs on.
pub const PROVIDER: &str = "runpod";
/// The Runpod account. The gateway holds no Runpod credential (row K7), so it names none.
pub const ACCOUNT: &str = "default";
/// The budget ledger the hosting contract attaches each pod to. No ledger is consulted yet.
pub const LEDGER: &str = "b10x-llm-gateway";
/// The reservation every pod is created under.
pub const RESERVATION: &str = "unmetered";
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

/// Opens connections to pods. The production one speaks TLS to the Runpod proxy
/// (story:live-runpod-wiring); a test's reaches a loopback pod.
pub trait PodConnector: Send + Sync {
    /// Opens one connection to the pod at `authority`, the `host[:port]` of the endpoint the
    /// pool reported. Its read and write timeouts are the connector's.
    ///
    /// # Errors
    /// The connection's own error; the gateway answers it `upstream-failed`.
    fn connect(&self, authority: &str) -> io::Result<Box<dyn RelayStream>>;
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
/// None for a loaded document; a `config:value` [`Refusal`] if a fixed value were invalid.
pub fn hosting_policy(deployment: &Deployment) -> Result<HostingPolicy, Refusal> {
    Ok(HostingPolicy {
        controller: identifier(CONTROLLER, "the controller")?,
        provider: identifier(PROVIDER, "the provider")?,
        account: identifier(ACCOUNT, "the account")?,
        ledger: identifier(LEDGER, "the ledger")?,
        max_active: u32::try_from(deployment.models.len()).unwrap_or(u32::MAX),
        max_lifetime_ms: MAX_LIFETIME_MS,
        lease_ms: LEASE_MS,
    })
}

/// The fixed authorization every pod is created under.
///
/// # Errors
/// A `config:value` [`Refusal`] if a fixed value were invalid.
pub fn compute_authorization() -> Result<ComputeAuthorization, Refusal> {
    Ok(ComputeAuthorization {
        ledger: identifier(LEDGER, "the ledger")?,
        reservation: identifier(RESERVATION, "the reservation")?,
    })
}

/// `idle_timeout_minutes` as the pool's idle window. 0 is no idle grace (row K27): the pool takes
/// no zero window, so it is 1 ms, and the pod is stopped by the first cleanup pass that finds it
/// quiet, never sooner than its measured cold start.
pub const fn idle_timeout_ms(minutes: u64) -> u64 {
    minutes.saturating_mul(60_000)
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
    RunpodPool::new(
        hosting_policy(deployment)?,
        Arc::new(LeaseRegistry::default()),
        transport,
        runpod_models(deployment)?,
        clock,
    )
    .map_err(|error| {
        Refusal::new(
            StartupRefusal::ConfigValue,
            format!("the Runpod pool cannot be composed: {error}"),
        )
    })
}

/// What one relayed model needs at acquisition.
struct Relayed {
    alias: Identifier,
    bearer: Arc<TargetBearer>,
    /// How long a request waits for the model's pod: `start_wait_seconds` (row K28).
    wait: Duration,
}

/// The gateway's targets: the pod the pool hands out for each alias.
pub(crate) struct PoolTargets<T> {
    pool: Arc<RunpodPool<T>>,
    authorization: ComputeAuthorization,
    models: BTreeMap<String, Relayed>,
    connector: Arc<dyn PodConnector>,
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
    ) -> Result<Self, Refusal> {
        let mut models = BTreeMap::new();
        for (alias, model) in &deployment.models {
            let at = format!("models.{alias}.vllm_api_key_file");
            let key = keys.get(alias).ok_or_else(|| {
                Refusal::new(
                    StartupRefusal::ConfigValue,
                    format!("{at} is required to relay to a Runpod pod"),
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
                    wait: Duration::from_secs(model.start_wait_seconds),
                },
            );
        }
        Ok(Self {
            pool,
            authorization: compute_authorization()?,
            models,
            connector,
        })
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

impl<T: RunpodTransport + Send + 'static> RelayTargets for PoolTargets<T> {
    /// Asks the pool for the model's ready pod, and while it is starting asks again until the
    /// model's `start_wait_seconds` have passed.
    fn acquire(&self, alias: &str) -> Option<Box<dyn RelayTarget>> {
        let relayed = self.models.get(alias)?;
        let deadline = Instant::now() + relayed.wait;
        loop {
            match self.pool.ensure(&relayed.alias, &self.authorization) {
                Ok(lease) => {
                    let authority = authority(lease.endpoint()?)?.to_owned();
                    return Some(Box::new(PodTarget {
                        authority,
                        bearer: Arc::clone(&relayed.bearer),
                        connector: Arc::clone(&self.connector),
                        _lease: lease,
                    }));
                }
                Err(PoolError::Starting | PoolError::Stopping) if Instant::now() < deadline => {
                    thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
                }
                Err(_) => return None,
            }
        }
    }

    /// Nothing is dropped on one failed request: the pool's own observation retires a pod that
    /// stopped serving, crash-loops, exited or refused its key, and the next acquisition starts
    /// its replacement.
    fn invalidate(&self, _alias: &str, _authority: &str) {}
}

/// One pod, held through its [`StreamLease`] until the relayed answer has ended.
struct PodTarget {
    authority: String,
    bearer: Arc<TargetBearer>,
    connector: Arc<dyn PodConnector>,
    _lease: StreamLease,
}

impl RelayTarget for PodTarget {
    fn authority(&self) -> &str {
        &self.authority
    }

    fn bearer(&self) -> Option<&TargetBearer> {
        Some(&self.bearer)
    }

    fn connect(&self) -> io::Result<Box<dyn RelayStream>> {
        self.connector.connect(&self.authority)
    }
}

#[cfg(test)]
mod tests {
    use super::{authority, idle_timeout_ms, secret_name};

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
}
