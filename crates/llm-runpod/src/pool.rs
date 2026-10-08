//! One pool of Runpod vLLM deployments, one per declared model, driven through the hosting
//! controller.
//!
//! Ported from `PodManager` in llmgw `src/runpod.rs`: `ensure_ready`'s single starter,
//! `start`'s readiness loop and crash-loop termination, `reap_once`'s idle reaper with its
//! measured cold-start floor, `sweep_orphans`, and `Flight`/`ReadyLease`'s in-flight accounting.
//! The difference is that every mutation now goes through `llm_provision::Controller`, so each
//! pod has an ownership lease, a fencing epoch, a budget authorization and an explicit stop
//! obligation, and adoption is decided by exact identity rather than by name.
//!
//! The pool is synchronous and holds one mutex across a whole step, provider calls included.
//! That is what makes startup single-flight: the caller that finds no live deployment declares
//! and submits one before any other caller can look.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use llm_provision::{
    ComputeAuthorization, Controller, DeploymentRecord, DeploymentSpec, HostingCommand,
    HostingError, HostingPolicy, HostingProvider, HostingReceipt, HostingView, Identifier,
    LeaseRegistry, Phase, ProviderState, ResourceKey, StopReason,
};

use crate::{
    ConfigError, RunpodModel, RunpodProvider, RunpodTransport, Unserviceable, request::in_namespace,
};

/// A trusted Unix-millisecond clock. It must never move backwards.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// A clock that moves only when told to. Clones share one instant.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    pub fn new(start_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(start_ms)))
    }

    pub fn advance(&self, ms: u64) {
        let _previous = self
            .0
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
                Some(now.saturating_add(ms))
            });
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Why the pool did not hand out a ready endpoint. Fixed codes, no upstream text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolError {
    /// No model is declared under this alias. Runpod was not contacted.
    UnknownModel,
    /// A model declaration is unusable.
    InvalidModel(ConfigError),
    /// A pod is starting, or its readiness is not known right now. Ask again.
    Starting,
    /// The deployment owes a stop that has not been confirmed. Nothing new is created meanwhile.
    Stopping,
    /// Every declared GPU type was refused. Nothing was created.
    NoCapacity,
    /// The pod crash-looped and was terminated. The next request starts a fresh one.
    CrashLoop,
    /// The pod refused its vLLM key and was terminated.
    CredentialRefused,
    /// The pod did not serve within its startup deadline and was terminated.
    StartupDeadline,
    /// Runpod reported the pod exited. It still exists and bills, so it was terminated.
    PodExited,
    /// The pod served, then answered "not ready" for longer than its startup deadline since it
    /// last served, and was terminated.
    StoppedServing,
    /// The hosting controller refused.
    Hosting(HostingError),
}

impl PoolError {
    /// The stable kebab-case code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownModel => "unknown-model",
            Self::InvalidModel(_) => "invalid-model",
            Self::Starting => "starting",
            Self::Stopping => "stopping",
            Self::NoCapacity => "no-capacity",
            Self::CrashLoop => "crash-loop",
            Self::CredentialRefused => "credential-refused",
            Self::StartupDeadline => "startup-deadline",
            Self::PodExited => "pod-exited",
            Self::StoppedServing => "stopped-serving",
            Self::Hosting(_) => "hosting",
        }
    }
}

impl std::fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidModel(error) => write!(formatter, "invalid Runpod model: {error}"),
            Self::Hosting(error) => write!(formatter, "hosting refused: {error}"),
            other => formatter.write_str(match other {
                Self::UnknownModel => "no model is declared under this alias",
                Self::Starting => "the model is starting",
                Self::Stopping => "the deployment is being stopped",
                Self::NoCapacity => "no declared GPU type had capacity",
                Self::CrashLoop => "vLLM crash-looped; the pod was terminated",
                Self::CredentialRefused => "the pod refused its vLLM key; the pod was terminated",
                Self::StartupDeadline => {
                    "the pod did not serve within its startup deadline; it was terminated"
                }
                Self::PodExited => "the pod exited; it was terminated",
                Self::StoppedServing => "the pod stopped serving; it was terminated",
                _ => "",
            }),
        }
    }
}

impl std::error::Error for PoolError {}

/// What one cleanup pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanupReport {
    /// Deployments stopped for sitting idle past their limit.
    pub idle: Vec<Identifier>,
    /// Deployments stopped because their model is no longer declared.
    pub retired: Vec<Identifier>,
    /// Deployments stopped because the pod crash-looped, refused its key or missed its deadline.
    pub replaced: Vec<Identifier>,
    /// Pod ids terminated because this controller created them and no live record holds them.
    pub orphans: Vec<Identifier>,
}

#[derive(Debug, Default)]
struct Usage {
    in_flight: AtomicU64,
    last_used_ms: AtomicU64,
}

/// A ready endpoint plus the in-flight accounting that keeps the idle reaper away from it.
///
/// Hold it for as long as the response, stream included, is being relayed. Dropping it starts
/// the idle window.
pub struct StreamLease {
    usage: Arc<Usage>,
    clock: Arc<dyn Clock>,
    deployment: Identifier,
    endpoint: Option<String>,
    served_model: Option<Identifier>,
}

impl StreamLease {
    pub fn deployment(&self) -> &Identifier {
        &self.deployment
    }
    /// The endpoint the provider reported. `None` when it reported none.
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }
    /// The model the pod itself says it serves. `None` when it did not say.
    pub fn served_model(&self) -> Option<&Identifier> {
        self.served_model.as_ref()
    }
}

impl std::fmt::Debug for StreamLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StreamLease")
            .field("deployment", &self.deployment)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Drop for StreamLease {
    fn drop(&mut self) {
        self.usage.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.usage
            .last_used_ms
            .fetch_max(self.clock.now_ms(), Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct Slot {
    current: Option<Identifier>,
    usage: Arc<Usage>,
    /// How long this model's last cold start took. `None` until one was measured here: an
    /// adopted pod was never started by this process, so nothing is inferred from it.
    measured_start_ms: Option<u64>,
    /// When the current deployment's startup deadline started running.
    deadline_from_ms: u64,
    ready_seen: bool,
    /// When the current deployment was last observed ready. `None` until it has been.
    last_ready_ms: Option<u64>,
    created_here: bool,
}

impl Slot {
    fn new(now_ms: u64) -> Self {
        let usage = Usage::default();
        usage.last_used_ms.store(now_ms, Ordering::SeqCst);
        Self {
            current: None,
            usage: Arc::new(usage),
            measured_start_ms: None,
            deadline_from_ms: now_ms,
            ready_seen: false,
            last_ready_ms: None,
            created_here: false,
        }
    }

    fn idle_limit_ms(&self, model: &RunpodModel) -> u64 {
        // Reaping is instant and starting is not: a timeout shorter than the start it causes
        // spends minutes of waiting to save seconds of idle.
        model
            .idle_timeout_ms
            .max(self.measured_start_ms.unwrap_or(0))
    }
}

/// Why a live deployment is being taken down this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retire {
    Unconfigured,
    Unserviceable(Unserviceable),
    StartupDeadline,
    StoppedServing,
}

impl Retire {
    const fn error(self) -> PoolError {
        match self {
            Self::Unconfigured => PoolError::UnknownModel,
            Self::Unserviceable(Unserviceable::CrashLoop) => PoolError::CrashLoop,
            Self::Unserviceable(Unserviceable::CredentialRefused) => PoolError::CredentialRefused,
            Self::Unserviceable(Unserviceable::Exited) => PoolError::PodExited,
            Self::StartupDeadline => PoolError::StartupDeadline,
            Self::StoppedServing => PoolError::StoppedServing,
        }
    }
}

struct Inner<T> {
    policy: HostingPolicy,
    controller: Controller,
    provider: RunpodProvider<T>,
    models: BTreeMap<Identifier, RunpodModel>,
    slots: BTreeMap<Identifier, Slot>,
    /// Pods the restored snapshot's previous lease holder owned: exact key, then that holder
    /// and the epoch it held. Runpod cannot rewrite the owner a pod was created with, so after
    /// a takeover these pods still name their previous owner; a pod whose tag still names
    /// exactly that owner is this pool's to stop.
    inherited: BTreeMap<ResourceKey, (Identifier, u64)>,
}

/// Distinguishes request ids issued in one process at one instant.
static REQUEST_NONCE: AtomicU64 = AtomicU64::new(0);

/// The most bytes of readable text a request id carries in front of its digest.
const READABLE_PREFIX_BYTES: usize = 64;

/// The idempotency key of one create: a short readable prefix and a fixed-length digest.
///
/// The digest covers a length-prefixed encoding of controller, alias, generation, instant and
/// nonce, so two different tuples never encode alike — `c-qwen` / `x` and `c` / `qwen-x` are
/// different encodings although their dash-joined forms agree. The readable prefix is for
/// people only and is cut at [`READABLE_PREFIX_BYTES`]; uniqueness rests on the digest. The
/// result is at most 96 bytes whatever the lengths of the controller id and alias, so it always
/// fits the 256-byte identifier limit.
///
/// The digest is two keyed passes of the standard library's hasher, 128 bits in all. It is not
/// a cryptographic hash and does not need to be: its inputs are this controller's own values,
/// not an adversary's, and what it has to avoid is an accidental collision.
fn request_id(controller: &str, alias: &str, generation: usize, now: u64, nonce: u64) -> String {
    use std::hash::{DefaultHasher, Hasher};
    let encoding = format!(
        "{}:{controller}{}:{alias}{generation}:{now}:{nonce}",
        controller.len(),
        alias.len()
    );
    let mut halves = [0_u64; 2];
    for (pass, half) in (0_u8..).zip(halves.iter_mut()) {
        let mut hasher = DefaultHasher::new();
        hasher.write_u8(pass);
        hasher.write(encoding.as_bytes());
        *half = hasher.finish();
    }
    // Identifiers are printable ASCII, so any byte offset is a character boundary.
    let mut readable = format!("request-{controller}-{alias}-{generation}-");
    readable.truncate(READABLE_PREFIX_BYTES);
    format!("{readable}{:016x}{:016x}", halves[0], halves[1])
}

/// Runpod vLLM deployments for a set of declared models.
pub struct RunpodPool<T> {
    inner: Mutex<Inner<T>>,
    clock: Arc<dyn Clock>,
}

fn validate(models: &BTreeMap<Identifier, RunpodModel>) -> Result<(), PoolError> {
    for model in models.values() {
        model.validate().map_err(PoolError::InvalidModel)?;
    }
    Ok(())
}

impl<T: RunpodTransport> RunpodPool<T> {
    /// A pool over an empty scope.
    ///
    /// # Errors
    ///
    /// [`PoolError::InvalidModel`] for an unusable declaration, and
    /// [`PoolError::Hosting`] for an unusable policy.
    pub fn new(
        policy: HostingPolicy,
        leases: Arc<LeaseRegistry>,
        transport: T,
        models: BTreeMap<Identifier, RunpodModel>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, PoolError> {
        Self::restore(policy, leases, transport, models, Vec::new(), clock)
    }

    /// A pool reopened over durable records after a restart.
    ///
    /// Adoption is by record, never by name: a pod is taken back only when the restored record
    /// names its exact identity or its request id, and the provider does not label it for
    /// another owner. A pod this controller created and no record holds is an orphan, and the
    /// next [`Self::reap`] terminates it.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], plus whatever `Controller::restore` refuses.
    pub fn restore(
        policy: HostingPolicy,
        leases: Arc<LeaseRegistry>,
        transport: T,
        models: BTreeMap<Identifier, RunpodModel>,
        snapshot: Vec<DeploymentRecord>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, PoolError> {
        validate(&models)?;
        let now = clock.now_ms();
        let live: Vec<_> = snapshot
            .iter()
            .filter(|record| !record.phase.terminal())
            .map(|record| (record.requested.model.clone(), record.deployment.clone()))
            .collect();
        let inherited = snapshot
            .iter()
            .filter(|record| !record.phase.terminal())
            .filter_map(|record| {
                let lease = record.lease.as_ref()?;
                let observed = record.observed.as_ref()?;
                (lease.owner != policy.controller)
                    .then(|| (observed.key.clone(), (lease.owner.clone(), lease.epoch)))
            })
            .collect();
        let controller = Controller::restore(policy.clone(), leases, snapshot, now)
            .map_err(PoolError::Hosting)?;
        let provider = RunpodProvider::new(
            transport,
            policy.provider.clone(),
            policy.account.clone(),
            models.clone(),
            Arc::clone(&clock),
        );
        let mut slots: BTreeMap<_, _> = models
            .keys()
            .map(|alias| (alias.clone(), Slot::new(now)))
            .collect();
        for (alias, deployment) in live {
            if let Some(slot) = slots.get_mut(&alias) {
                slot.current = Some(deployment);
            }
        }
        Ok(Self {
            inner: Mutex::new(Inner {
                policy,
                controller,
                provider,
                models,
                slots,
                inherited,
            }),
            clock,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hands out a ready endpoint for a model, starting its pod when none is live.
    ///
    /// Each call makes at most one step: it observes, retires what has to go, and then either
    /// serves, reports that the pod is still starting, or submits exactly one create. A caller
    /// that is told [`PoolError::Starting`] asks again; it never causes a second pod.
    ///
    /// # Errors
    ///
    /// Any [`PoolError`]; see each variant.
    pub fn ensure(
        &self,
        alias: &Identifier,
        authorization: &ComputeAuthorization,
    ) -> Result<StreamLease, PoolError> {
        self.hand_out(alias, Some(authorization))
    }

    /// Hands out the model's pod only if it is ready now, and never starts one: when no pod is
    /// live, or the live one is not ready, it is [`PoolError::Starting`] and nothing was
    /// submitted. For a caller that is stopping and may still use what already serves.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure`]; [`PoolError::Starting`] also when no pod is live.
    pub fn ensure_running(&self, alias: &Identifier) -> Result<StreamLease, PoolError> {
        self.hand_out(alias, None)
    }

    /// [`Self::ensure`] when `authorization` is given; without it, nothing is started.
    fn hand_out(
        &self,
        alias: &Identifier,
        authorization: Option<&ComputeAuthorization>,
    ) -> Result<StreamLease, PoolError> {
        let mut inner = self.lock();
        if !inner.models.contains_key(alias) {
            return Err(PoolError::UnknownModel);
        }
        let now = self.clock.now_ms();
        let retired = inner.step(now)?;
        let current = inner.slots.get(alias).and_then(|slot| slot.current.clone());
        if let Some(deployment) = &current
            && let Some(reason) = retired.get(deployment)
        {
            inner.forget_terminal(alias);
            return Err(reason.error());
        }
        inner.forget_terminal(alias);
        let current = inner.slots.get(alias).and_then(|slot| slot.current.clone());
        let Some(deployment) = current else {
            return match authorization {
                Some(authorization) => inner.start(now, alias, authorization),
                None => Err(PoolError::Starting),
            };
        };
        let record = inner.record(&deployment).ok_or(PoolError::Starting)?;
        match record.phase {
            Phase::StopRequired => Err(PoolError::Stopping),
            Phase::Requested | Phase::Active if ready_now(&record, now) => {
                let observed = record.observed.as_ref();
                let slot = inner.slots.get(alias).ok_or(PoolError::UnknownModel)?;
                slot.usage.in_flight.fetch_add(1, Ordering::SeqCst);
                slot.usage.last_used_ms.fetch_max(now, Ordering::SeqCst);
                Ok(StreamLease {
                    usage: Arc::clone(&slot.usage),
                    clock: Arc::clone(&self.clock),
                    deployment,
                    endpoint: observed.and_then(|observed| observed.endpoint.clone()),
                    served_model: observed.and_then(|observed| observed.served_model.clone()),
                })
            }
            _ => Err(PoolError::Starting),
        }
    }

    /// One cleanup pass: retire, reap idle pods, confirm owed stops and sweep orphans.
    ///
    /// These are the billing guards. Runpod never terminates a pod on its own, so a pod
    /// nothing here reaches runs until somebody notices the bill.
    ///
    /// # Errors
    ///
    /// [`PoolError::Hosting`] when the controller refuses the clock.
    pub fn reap(&self) -> Result<CleanupReport, PoolError> {
        let mut inner = self.lock();
        let now = self.clock.now_ms();
        let retired = inner.step(now)?;
        let mut report = CleanupReport::default();
        for (deployment, reason) in retired {
            if reason == Retire::Unconfigured {
                report.retired.push(deployment);
            } else {
                report.replaced.push(deployment);
            }
        }
        report.idle = inner.reap_idle(now);
        inner.submit_owed_stops(now);
        report.orphans = inner.sweep_orphans();
        let aliases: Vec<_> = inner.slots.keys().cloned().collect();
        for alias in aliases {
            inner.forget_terminal(&alias);
        }
        Ok(report)
    }

    /// Drops a model's current endpoint after a request through it failed (row W7): when the
    /// current deployment's reported endpoint satisfies `failed`, its stop is requested and
    /// submitted at once, so the next [`Self::ensure`] starts a replacement. A deployment that
    /// already changed, or whose endpoint does not match, is left alone: a stale report never
    /// stops the replacement. Returns whether a stop was requested.
    ///
    /// # Errors
    ///
    /// [`PoolError::UnknownModel`] for an undeclared alias, and [`PoolError::Hosting`] when the
    /// controller refuses the clock.
    pub fn invalidate(
        &self,
        alias: &Identifier,
        failed: impl Fn(&str) -> bool,
    ) -> Result<bool, PoolError> {
        let mut inner = self.lock();
        if !inner.models.contains_key(alias) {
            return Err(PoolError::UnknownModel);
        }
        let now = self.clock.now_ms();
        let current = inner.slots.get(alias).and_then(|slot| slot.current.clone());
        let Some(record) = current.and_then(|deployment| inner.record(&deployment)) else {
            return Ok(false);
        };
        let matches = record
            .observed
            .as_ref()
            .and_then(|observed| observed.endpoint.as_deref())
            .is_some_and(failed);
        if !matches || !matches!(record.phase, Phase::Requested | Phase::Active) {
            return Ok(false);
        }
        let requested = inner
            .apply(
                now,
                HostingCommand::RequestStop {
                    deployment: record.deployment.clone(),
                },
            )
            .is_ok();
        inner.submit_owed_stops(now);
        inner.forget_terminal(alias);
        Ok(requested)
    }

    /// The controller's view. Contacts nobody.
    pub fn view(&self) -> HostingView {
        self.lock().controller.view()
    }

    /// The durable records, as [`Self::restore`] takes them back.
    pub fn snapshot(&self) -> Vec<DeploymentRecord> {
        self.lock().controller.snapshot()
    }
}

/// Whether a record was observed ready at this very step. An older observation is not current.
fn ready_now(record: &DeploymentRecord, now: u64) -> bool {
    record
        .observed
        .as_ref()
        .is_some_and(|observed| observed.observed_at_ms == now && observed.is_ready())
}

impl<T: RunpodTransport> Inner<T> {
    fn apply(&mut self, now: u64, command: HostingCommand) -> Result<HostingReceipt, HostingError> {
        self.controller.apply(now, &mut self.provider, command)
    }

    fn record(&self, deployment: &Identifier) -> Option<DeploymentRecord> {
        self.controller
            .view()
            .deployments
            .into_iter()
            .find(|record| &record.deployment == deployment)
    }

    fn live_records(&self) -> Vec<DeploymentRecord> {
        self.controller
            .view()
            .deployments
            .into_iter()
            .filter(|record| !record.phase.terminal())
            .collect()
    }

    /// Clears a slot whose deployment has reached a terminal phase.
    fn forget_terminal(&mut self, alias: &Identifier) {
        let terminal = self
            .slots
            .get(alias)
            .and_then(|slot| slot.current.as_ref())
            .and_then(|deployment| self.record(deployment))
            .is_none_or(|record| record.phase.terminal());
        if terminal && let Some(slot) = self.slots.get_mut(alias) {
            slot.current = None;
        }
    }

    /// Keeps a live claim on every record that may hold a pod, so its stop stays reachable.
    ///
    /// A renewal that fails because the claim ran out is followed by a reacquisition: the lease
    /// expiry already opened a stop obligation, and that obligation can only be carried out
    /// under a live claim.
    fn hold_leases(&mut self, now: u64) -> Result<(), PoolError> {
        for record in self.live_records() {
            if record.phase == Phase::Declared {
                continue;
            }
            let deployment = record.deployment.clone();
            let renewed = record.lease.is_some()
                && self
                    .apply(
                        now,
                        HostingCommand::Renew {
                            deployment: deployment.clone(),
                        },
                    )
                    .is_ok();
            if !renewed {
                match self.apply(now, HostingCommand::Acquire { deployment }) {
                    Ok(_) | Err(HostingError::LeaseHeld) => {}
                    Err(other) => return Err(PoolError::Hosting(other)),
                }
            }
        }
        Ok(())
    }

    /// Observes, retires what has to go, and submits every owed stop.
    fn step(&mut self, now: u64) -> Result<BTreeMap<Identifier, Retire>, PoolError> {
        self.hold_leases(now)?;
        self.apply(now, HostingCommand::Observe)
            .map_err(PoolError::Hosting)?;
        let mut retired = BTreeMap::new();
        for record in self.live_records() {
            // Nothing submitted yet, or a stop already owed: neither is retired again. A
            // record that owes a stop is reported as `Stopping` until the stop is confirmed.
            if matches!(record.phase, Phase::Declared | Phase::StopRequired) {
                continue;
            }
            let alias = record.requested.model.clone();
            if let Some(reason) = self.retire_reason(&record, &alias, now) {
                // Refused only for a phase that cannot owe a stop; nothing to do then.
                let _refused = self.apply(
                    now,
                    HostingCommand::RequestStop {
                        deployment: record.deployment.clone(),
                    },
                );
                retired.insert(record.deployment.clone(), reason);
            }
        }
        self.submit_owed_stops(now);
        self.stop_inherited();
        Ok(retired)
    }

    /// Terminates an inherited pod whose record the contract parked in `ownership-lost`.
    ///
    /// The controller refuses to stop a resource labelled for another owner, and it is right
    /// to: in general that label is the only evidence of whose resource it is. The exception is
    /// the label this pool inherited — the snapshot's previous lease holder, which Runpod gives
    /// no way to relabel. Only a pod listed under the record's exact key whose tag still names
    /// that holder is terminated; a pod labelled for
    /// anyone else is never touched. The record is discharged by the next listing, which no
    /// longer holds the pod.
    fn stop_inherited(&mut self) {
        let candidates: Vec<_> = self
            .live_records()
            .into_iter()
            .filter(|record| {
                record.phase.owes_a_stop() && record.stop_reason == Some(StopReason::OwnershipLost)
            })
            .filter_map(|record| {
                let key = record.observed?.key;
                let holder = self.inherited.get(&key)?.clone();
                Some((key, holder))
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let inventory = self.provider.inventory();
        for (key, (owner, epoch)) in candidates {
            // Owner only. An epoch comparison would decide nothing: the key includes the pod id
            // and Runpod cannot retag a pod, so an inherited key never carries a newer epoch.
            let still_theirs = inventory
                .exact(&key)
                .is_some_and(|resource| resource.owner.as_ref() == Some(&owner));
            if still_theirs {
                let _outcome = self.provider.stop(&key, epoch);
            }
        }
    }

    fn retire_reason(
        &mut self,
        record: &DeploymentRecord,
        alias: &Identifier,
        now: u64,
    ) -> Option<Retire> {
        let Some(model) = self.models.get(alias) else {
            return Some(Retire::Unconfigured);
        };
        if let Some(unserviceable) = record
            .observed
            .as_ref()
            .and_then(|observed| self.provider.unserviceable(&observed.key))
        {
            return Some(Retire::Unserviceable(unserviceable));
        }
        let deadline_ms = model.startup_deadline_ms;
        let slot = self.slots.get_mut(alias)?;
        if slot.current.as_ref() != Some(&record.deployment) {
            return None;
        }
        if ready_now(record, now) {
            if !slot.ready_seen && slot.created_here {
                slot.measured_start_ms = Some(now.saturating_sub(slot.deadline_from_ms));
            }
            if !slot.ready_seen {
                // A pod is idle only from the moment it could first serve: the request that
                // started it may still be waiting for it, and must find it standing.
                slot.usage.last_used_ms.fetch_max(now, Ordering::SeqCst);
            }
            slot.ready_seen = true;
            slot.last_ready_ms = Some(now);
            return None;
        }
        if let Some(last_ready) = slot.last_ready_ms {
            // A pod that served and now answers a definite "not ready" gets the same bound a
            // start gets, counted from the last time it served. Unknown readiness — no answer,
            // or no observation at this step — neither starts nor ends that clock.
            let definitely_unready = record.observed.as_ref().is_some_and(|observed| {
                observed.observed_at_ms == now && observed.ready == Some(false)
            });
            return (definitely_unready && now.saturating_sub(last_ready) > deadline_ms)
                .then_some(Retire::StoppedServing);
        }
        (!slot.ready_seen && now.saturating_sub(slot.deadline_from_ms) > deadline_ms)
            .then_some(Retire::StartupDeadline)
    }

    /// Submits the stop mutation for every record that owes one and can be addressed.
    fn submit_owed_stops(&mut self, now: u64) {
        for record in self.live_records() {
            let addressable =
                record.observed.is_some() && record.stop_reason != Some(StopReason::OwnershipLost);
            if record.phase.owes_a_stop() && addressable {
                // A refused or unconfirmed stop leaves the obligation open and visible in
                // `view().totals.stop_required`; the next pass submits it again.
                let _outcome = self.apply(
                    now,
                    HostingCommand::Stop {
                        deployment: record.deployment,
                    },
                );
            }
        }
    }

    fn reap_idle(&mut self, now: u64) -> Vec<Identifier> {
        let mut idle = Vec::new();
        let candidates: Vec<_> = self
            .slots
            .iter()
            .filter_map(|(alias, slot)| {
                let model = self.models.get(alias)?;
                let deployment = slot.current.clone()?;
                let quiet = slot.usage.in_flight.load(Ordering::SeqCst) == 0;
                let last_used = slot.usage.last_used_ms.load(Ordering::SeqCst);
                let expired = now.saturating_sub(last_used) >= slot.idle_limit_ms(model);
                (quiet && expired).then_some(deployment)
            })
            .collect();
        for deployment in candidates {
            if self
                .record(&deployment)
                .is_some_and(|record| record.phase == Phase::Active)
                && self
                    .apply(
                        now,
                        HostingCommand::RequestStop {
                            deployment: deployment.clone(),
                        },
                    )
                    .is_ok()
            {
                idle.push(deployment);
            }
        }
        idle
    }

    /// Terminates every pod this controller created that no live record holds.
    ///
    /// Selected only when all of these hold: the name is in this adapter's namespace (never
    /// llmgw's), the pod carries this controller's owner tag, it is not terminated, and neither
    /// its exact identity nor its request id belongs to a live record. A pod without an owner
    /// tag, or with somebody else's, is never touched.
    fn sweep_orphans(&mut self) -> Vec<Identifier> {
        let live = self.live_records();
        let keys: BTreeSet<_> = live
            .iter()
            .filter_map(|record| {
                record
                    .observed
                    .as_ref()
                    .map(|observed| observed.key.clone())
            })
            .collect();
        // A request id protects a pod only for a record that holds no exact key yet: that is
        // the lost create whose pod has not been adopted. A record that already holds a key is
        // protected by the key, and a second pod carrying its request id is somebody's
        // duplicate, not its resource.
        let requests: BTreeSet<_> = live
            .iter()
            .filter(|record| record.observed.is_none())
            .map(|record| record.request_id.clone())
            .collect();
        let inventory = self.provider.inventory();
        let mut orphans = Vec::new();
        for resource in inventory.resources {
            let ours = resource.owner.as_ref() == Some(&self.policy.controller);
            let recorded = keys.contains(&resource.key)
                || resource
                    .request_id
                    .as_ref()
                    .is_some_and(|request| requests.contains(request));
            if !in_namespace(resource.key.name.as_str())
                || !ours
                || recorded
                || resource.state == ProviderState::Terminated
            {
                continue;
            }
            let outcome = self
                .provider
                .stop(&resource.key, resource.epoch.unwrap_or(0));
            if outcome.evidence.is_some() {
                orphans.push(resource.key.incarnation);
            }
        }
        orphans
    }

    /// Declares, claims and submits one new deployment for a model.
    fn start(
        &mut self,
        now: u64,
        alias: &Identifier,
        authorization: &ComputeAuthorization,
    ) -> Result<StreamLease, PoolError> {
        let model = self.models.get(alias).ok_or(PoolError::UnknownModel)?;
        let generation = self
            .controller
            .view()
            .deployments
            .iter()
            .filter(|record| &record.requested.model == alias)
            .count()
            .saturating_add(1);
        let invalid = |_| PoolError::Hosting(HostingError::InvalidSpec);
        let deployment = Identifier::new(format!("{alias}-{generation}")).map_err(invalid)?;
        let nonce = REQUEST_NONCE.fetch_add(1, Ordering::SeqCst);
        let request_id = Identifier::new(request_id(
            self.policy.controller.as_str(),
            alias.as_str(),
            generation,
            now,
            nonce,
        ))
        .map_err(invalid)?;
        let spec = DeploymentSpec {
            id: deployment.clone(),
            provider: self.policy.provider.clone(),
            account: self.policy.account.clone(),
            model: alias.clone(),
            image: model.image.clone(),
            resource_name: alias.clone(),
            requested_lifetime_ms: self.policy.max_lifetime_ms,
        };
        self.apply(
            now,
            HostingCommand::Declare {
                spec,
                request_id,
                authorization: authorization.clone(),
            },
        )
        .map_err(PoolError::Hosting)?;
        let submitted = self
            .apply(
                now,
                HostingCommand::Acquire {
                    deployment: deployment.clone(),
                },
            )
            .and_then(|_| {
                self.apply(
                    now,
                    HostingCommand::Provision {
                        deployment: deployment.clone(),
                    },
                )
            });
        let receipt = match submitted {
            Ok(receipt) => receipt,
            Err(error) => {
                // Nothing was submitted, so nothing is owed; withdraw the declaration.
                let _withdrawn = self.apply(
                    now,
                    HostingCommand::Cancel {
                        deployment: deployment.clone(),
                    },
                );
                return Err(PoolError::Hosting(error));
            }
        };
        if receipt.phase == Some(Phase::Cancelled) {
            return Err(PoolError::NoCapacity);
        }
        if let Some(slot) = self.slots.get_mut(alias) {
            slot.current = Some(deployment);
            slot.deadline_from_ms = now;
            slot.ready_seen = false;
            slot.last_ready_ms = None;
            slot.created_here = true;
            // In-flight accounting belongs to one deployment. A lease still open on the one
            // this replaces keeps counting against that one, not against this.
            let usage = Usage::default();
            usage.last_used_ms.store(now, Ordering::SeqCst);
            slot.usage = Arc::new(usage);
        }
        Err(PoolError::Starting)
    }
}

#[cfg(test)]
mod tests {
    use super::{PoolError, Retire, request_id};

    #[test]
    fn request_ids_differ_across_controller_and_alias_splits() {
        let joined = request_id("c-qwen", "x", 1, 7, 0);
        let split = request_id("c", "qwen-x", 1, 7, 0);
        assert_ne!(
            joined, split,
            "the dash-joined forms agree; the encodings must not"
        );
        assert_eq!(request_id("c", "qwen-x", 1, 7, 0), split, "deterministic");
        for varied in [
            request_id("c", "qwen-x", 2, 7, 0),
            request_id("c", "qwen-x", 1, 8, 0),
            request_id("c", "qwen-x", 1, 7, 1),
        ] {
            assert_ne!(varied, split);
        }
    }

    #[test]
    fn a_request_id_always_fits_the_identifier_limit() {
        let longest = request_id(
            &"c".repeat(256),
            &"q".repeat(256),
            usize::MAX,
            u64::MAX,
            u64::MAX,
        );
        assert_eq!(longest.len(), 96);
        assert!(llm_provision::Identifier::new(longest).is_ok());
        assert!(
            request_id("controller-a", "qwen", 1, 1_000, 0)
                .starts_with("request-controller-a-qwen-1-")
        );
    }
    use crate::Unserviceable;

    #[test]
    fn every_retirement_reason_reports_its_own_refusal() {
        assert_eq!(Retire::Unconfigured.error(), PoolError::UnknownModel);
        assert_eq!(
            Retire::Unserviceable(Unserviceable::CrashLoop).error(),
            PoolError::CrashLoop
        );
        assert_eq!(
            Retire::Unserviceable(Unserviceable::CredentialRefused).error(),
            PoolError::CredentialRefused
        );
        assert_eq!(Retire::StartupDeadline.error(), PoolError::StartupDeadline);
        assert_eq!(Retire::StoppedServing.error(), PoolError::StoppedServing);
    }
}
