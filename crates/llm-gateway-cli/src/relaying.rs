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
    config::{Deployment, Model, Wire},
    keys::VllmKeys,
    refusal::{Refusal, StartupRefusal},
    serve::{Running, Stopped, inventory, owner_verifier},
};
use llm_gateway::{
    Gateway, Label, Relay, RelayModel, RelayStream, RelayTarget, RelayTargets, ShutdownReport,
    TargetBearer, TargetRefusal,
};
use llm_runpod::{
    Clock, ComputeAuthorization, Hold, HostingPolicy, Identifier, LeaseRegistry, PoolError,
    RunpodModel, RunpodPool, RunpodTransport, StreamLease,
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
/// How often a waiting request looks for the stop between two asks.
const STOP_CHECK: Duration = Duration::from_millis(20);

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
    /// How long a request waits for the model's pod: `request_hold_seconds` (row K28).
    hold: Duration,
}

/// The gateway's targets: the pod the pool hands out for each alias.
pub(crate) struct PoolTargets<T> {
    pool: Arc<RunpodPool<T>>,
    authorization: ComputeAuthorization,
    models: BTreeMap<String, Relayed>,
    connector: Arc<dyn PodConnector>,
    /// Set when the gateway starts to stop: a request still waiting for its pod gives up.
    stopping: Arc<AtomicBool>,
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
                    hold: Duration::from_secs(model.request_hold_seconds),
                },
            );
        }
        Ok(Self {
            pool,
            authorization: compute_authorization()?,
            models,
            connector,
            stopping,
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
    fn acquire(&self, alias: &str) -> Result<Box<dyn RelayTarget>, TargetRefusal> {
        let relayed = self.models.get(alias).ok_or(TargetRefusal::Unavailable)?;
        let deadline = Instant::now() + relayed.hold;
        let mut hold = Hold::default();
        loop {
            let stopping = self.stopping.load(Ordering::SeqCst);
            let asked = if stopping {
                self.pool.ensure_running(&relayed.alias)
            } else {
                self.pool
                    .ensure_held(&relayed.alias, &self.authorization, &mut hold)
            };
            match asked {
                Ok(lease) => {
                    let authority = lease
                        .endpoint()
                        .and_then(authority)
                        .ok_or(TargetRefusal::Unavailable)?
                        .to_owned();
                    return Ok(Box::new(PodTarget {
                        authority,
                        bearer: Arc::clone(&relayed.bearer),
                        connector: Arc::clone(&self.connector),
                        _lease: lease,
                    }));
                }
                Err(PoolError::Starting | PoolError::Stopping) if !stopping && !hold.lost() => {
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
                Err(_) => return Err(TargetRefusal::Unavailable),
            }
        }
    }

    /// A request through the pod at `authority` failed (row W7): the pool stops that pod if it
    /// is still the model's current one, so the next acquisition starts a replacement. A report
    /// about a pod already replaced changes nothing.
    fn invalidate(&self, alias: &str, failed: &str) {
        if let Some(relayed) = self.models.get(alias) {
            let _stopped = self.pool.invalidate(&relayed.alias, |endpoint| {
                authority(endpoint) == Some(failed)
            });
        }
    }
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
    cleanup: Cleanup,
    stopping: Arc<AtomicBool>,
}

impl Relaying {
    pub fn local_addr(&self) -> SocketAddr {
        self.running.local_addr()
    }

    /// Drains and stops the gateway gracefully now, then stops the cleanup pass. A request
    /// still waiting for its pod is answered `target-unavailable` at once.
    pub fn shutdown(self) -> ShutdownReport {
        self.stopping.store(true, Ordering::SeqCst);
        let report = self.running.shutdown();
        self.cleanup.stop();
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

const fn relay_wire(wire: Wire) -> llm_gateway::Wire {
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
/// The shipped binary has no Runpod transport and never calls this (story:live-runpod-wiring);
/// it is the seam a test composes with `EmulatedRunpod` and loopback pods.
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
    let verifier = owner_verifier(deployment)?;
    let vllm_keys = crate::keys::vllm_keys(deployment)?;
    let inventory = inventory(deployment)?;
    let pool = Arc::new(runpod_pool(deployment, transport, clock)?);
    let stopping = Arc::new(AtomicBool::new(false));
    let targets = PoolTargets::new(
        deployment,
        &vllm_keys,
        Arc::clone(&pool),
        pods,
        Arc::clone(&stopping),
    )?;
    let mut models = Vec::with_capacity(deployment.models.len());
    for (alias, model) in &deployment.models {
        let wires = model.wires.iter().copied().map(relay_wire).collect();
        // The pod serves the model under its alias (`--served-model-name`), so the upstream
        // name is the alias too.
        let relayed = RelayModel::new(relay_label(alias)?, relay_label(alias)?, wires);
        models.push(relayed.map_err(|error| {
            Refusal::new(
                StartupRefusal::ConfigValue,
                format!("models.{alias}: {error}"),
            )
        })?);
    }
    let relay = Relay::new(models, Arc::new(targets))
        .map_err(|error| Refusal::new(StartupRefusal::ConfigValue, format!("models: {error}")))?;
    let signals = Running::install_signals()?;
    let bind = deployment.gateway.bind;
    let handle = Gateway::bind_with_relay(
        deployment.gateway.clone(),
        Arc::new(verifier),
        inventory,
        relay,
    )
    .map_err(|error| Refusal::new(StartupRefusal::ListenBind, format!("{bind}: {error}")))?;
    // One pass before serving: it sweeps the pods a previous run of this controller left. A
    // refused pass changes nothing, and the next one runs on time.
    let _swept = pool.reap();
    let cleanup = Cleanup::start(CLEANUP_INTERVAL, move || {
        let _report = pool.reap();
    });
    handle.mark_ready();
    Ok(Relaying {
        running: Running::from_parts(handle, signals, vllm_keys),
        cleanup,
        stopping,
    })
}

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
