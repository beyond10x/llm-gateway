//! Runpod adapter conformance observations.
//!
//! This module translates an authored fixture program into calls on the real
//! `b10x-llm-runpod` public API — a `RunpodPool` driving a `RunpodProvider` over the crate's own
//! in-process `EmulatedRunpod` — and reports what those calls returned. It reimplements no
//! lifecycle rule, reads no suite and branches on no scenario name: every answer, pod call,
//! lease and cleanup below comes back out of the pool or the emulator. The one fixture it adds is
//! a transport wrapper for two arrangements the emulator cannot script on its own: a pod that
//! already served keeps answering "not ready" (the crate's `tests/adversary2.rs` uses the same
//! wrapper), and every create takes a while of the pool's clock, as a `connectors` write does
//! (the crate's `tests/adversary_w12_profiles.rs` uses the same wrapper).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Barrier, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use ess_conformance::target::TargetError;
use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    AccountSpend, CleanupReport, CloudType, CreateAnswer, EmulatedRunpod, ManualClock, ModelStatus,
    NetworkVolume, PodListing, PodRequest, PodStatus, PoolError, Probe, ProbeTarget, RunpodModel,
    RunpodPool, RunpodProfile, RunpodTransport, SpendPolicy, SpendRefusal, StreamLease,
    TAG_REQUEST, TerminateAnswer, Thinking, VllmSettings,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::target::Observed;

/// Every view name this domain answers through `query_view`.
pub const VIEWS: &[&str] = &["llm-gateway.runpod.LastExecution"];

const MAX_PROGRAM_BYTES: usize = 64 * 1024;
const MAX_STEPS: usize = 128;
const MAX_CALLERS: usize = 32;
const MAX_ATTEMPTS: usize = 2_000;
/// How far `until_ready` and `hold` move the clock after each `starting` answer.
const RETRY_STEP_MS: u64 = 1_000;

/// Observe one command of this domain, or `None` when the command belongs to another.
pub fn observe(command: &str, input: &Value) -> Option<Result<Observed, TargetError>> {
    if command != "llm-gateway.runpod.Exercise" {
        return None;
    }
    Some(exercise(input))
}

fn unavailable(error: impl std::fmt::Display) -> TargetError {
    TargetError::unavailable("runpod observation", error.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exercise {
    program_json: String,
}

fn exercise(input: &Value) -> Result<Observed, TargetError> {
    let request: Exercise = serde_json::from_value(input.clone()).map_err(unavailable)?;
    Ok(Observed {
        facts: run(&request.program_json),
        view: "llm-gateway.runpod.LastExecution",
        event: "llm-gateway.runpod.Exercised",
        field: "valid_program",
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    models: BTreeMap<String, ModelInput>,
    #[serde(default)]
    max_active: Option<u32>,
    /// The pool's spend ceilings in USD (`SpendLedger`); absent is no ceiling.
    #[serde(default)]
    spend: Option<SpendInput>,
    steps: Vec<Step>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct SpendInput {
    daily_cap_usd: Option<f64>,
    total_cap_usd: Option<f64>,
}

/// Overrides on the crate test suite's reference model (`tests/runpod.rs` `model()`).
#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields, default)]
struct ModelInput {
    hf_model: Option<String>,
    image: Option<String>,
    gpu_types: Option<Vec<String>>,
    cloud_type: Option<String>,
    container_disk_gb: Option<u32>,
    cache: Option<CacheInput>,
    data_center_ids: Option<Vec<String>>,
    vllm: VllmInput,
    api_key_secret: Option<String>,
    startup_deadline_ms: Option<u64>,
    idle_timeout_ms: Option<u64>,
    crash_restart_limit: Option<u32>,
    crash_window_ms: Option<u64>,
    /// The model's declared price per hour in USD, which the spend check prices it by.
    price_per_hour_usd: Option<f64>,
    /// Ordered profiles. The first replaces the model's `gpu_types` and `vllm`; a missing vLLM
    /// field of a profile takes the runner's default.
    profiles: Option<Vec<ProfileInput>>,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct ProfileInput {
    name: String,
    gpu_types: Vec<String>,
    #[serde(default)]
    vllm: VllmInput,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct CacheInput {
    volume_id: String,
    mount_path: String,
}

#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields, default)]
struct VllmInput {
    max_model_len: Option<u32>,
    max_num_seqs: Option<u32>,
    gpu_memory_utilization: Option<f64>,
    thinking: Option<String>,
    reasoning_effort: Option<String>,
    sampling: Option<String>,
    extra_args: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(tag = "step", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    /// One `ensure`; a ready lease is dropped at once.
    Ensure {
        alias: String,
    },
    /// `ensure` until ready, moving the clock after each `starting`; the lease is kept.
    Hold {
        alias: String,
        #[serde(default)]
        max_attempts: Option<usize>,
    },
    /// Drop every kept lease.
    Release,
    /// One `wake` (the owner's lifecycle route): an `ensure`, a ready lease dropped, then the
    /// model's status.
    Wake {
        alias: String,
    },
    /// One `status`, which contacts nobody.
    Status {
        alias: String,
    },
    /// `ensure` until ready, moving the clock after each `starting`; the lease is dropped.
    UntilReady {
        alias: String,
        #[serde(default)]
        max_attempts: Option<usize>,
    },
    /// Several callers racing `ensure` at one instant, on real threads behind a barrier.
    Concurrent {
        alias: String,
        callers: usize,
    },
    Advance {
        ms: u64,
    },
    Reap,
    /// Rebuild the pool from its own durable records, optionally over different models.
    Restart {
        #[serde(default)]
        models: Option<BTreeMap<String, ModelInput>>,
    },
    RefuseGpu {
        gpu: String,
    },
    NoCapacityGpu {
        gpu: String,
    },
    LoseNextCreate {
        allocates: bool,
    },
    ReadyAfter {
        probes: u32,
    },
    Uptimes {
        script: Vec<u64>,
    },
    RefuseCredential {
        pod: String,
    },
    PartialListing {
        partial: bool,
    },
    ProbeUnreachable {
        unreachable: bool,
    },
    SetStatus {
        pod: String,
        status: String,
    },
    Vanish {
        pod: String,
    },
    Retag {
        pod: String,
        owner: String,
        epoch: u64,
    },
    /// A pod somebody else created. `request_of` copies the request id a record holds.
    InsertPod {
        name: String,
        env: BTreeMap<String, String>,
        #[serde(default)]
        request_of: Option<String>,
    },
    /// The pod answers "not ready" from now on, whatever the emulator would say.
    Wedge {
        pod: String,
    },
    /// Every create from now on takes `ms` of the pool's clock before it is answered.
    CreateTakes {
        ms: u64,
    },
    /// Spend reads from now on answer this reported spend, in USD.
    AccountSpend {
        daily_usd: f64,
        total_usd: f64,
    },
    /// Spend reads from now on answer from these billed amounts: today's spend is those at or
    /// after the UTC day's start the read asks for.
    Billed {
        entries: Vec<BilledInput>,
    },
}

/// One billed amount: `usd` at the pool clock's instant `at_ms`.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct BilledInput {
    at_ms: u64,
    usd: f64,
}

/// The emulator, plus the two arrangements it cannot script: a pod that stops serving, and a
/// create that takes time on the pool's clock.
#[derive(Clone)]
struct Fixture {
    runpod: EmulatedRunpod,
    wedged: Arc<Mutex<BTreeSet<String>>>,
    clock: ManualClock,
    create_ms: Arc<AtomicU64>,
}

impl Fixture {
    fn new(clock: ManualClock) -> Self {
        Self {
            runpod: EmulatedRunpod::new(),
            wedged: Arc::default(),
            clock,
            create_ms: Arc::default(),
        }
    }
}

impl RunpodTransport for Fixture {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.runpod.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.clock.advance(self.create_ms.load(Ordering::SeqCst));
        self.runpod.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.runpod.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let pod_id = target.pod_id;
        let answer = self.runpod.probe_ready(target);
        let wedged = self
            .wedged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(pod_id);
        if wedged { Probe::NotReady } else { answer }
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.runpod.container_started_at(pod_id)
    }
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        self.runpod.account_spend(day_start_ms)
    }
}

/// A USD amount of a program in micro-USD: 0, or a positive amount of at least one micro-USD.
fn amount(usd: f64) -> Result<u64, String> {
    if usd == 0.0 {
        return Ok(0);
    }
    llm_runpod::micro_usd(usd).ok_or_else(|| "fixture:invalid-amount".to_owned())
}

/// The program's spend ledger: its ceilings and every model's price.
fn spend_policy(
    spend: Option<&SpendInput>,
    models: &BTreeMap<String, ModelInput>,
) -> Result<SpendPolicy, String> {
    let cap = |usd: Option<f64>| usd.map(amount).transpose();
    let mut prices = BTreeMap::new();
    for (alias, model) in models {
        if let Some(price) = model.price_per_hour_usd {
            prices.insert(ident(alias)?, amount(price)?);
        }
    }
    Ok(SpendPolicy {
        daily_cap_micro_usd: cap(spend.and_then(|spend| spend.daily_cap_usd))?,
        total_cap_micro_usd: cap(spend.and_then(|spend| spend.total_cap_usd))?,
        price_per_hour_micro_usd: prices,
    })
}

fn ident(value: &str) -> Result<Identifier, String> {
    Identifier::new(value).map_err(|_| "fixture:invalid-identifier".to_owned())
}

fn build_vllm(vllm: &VllmInput) -> Result<VllmSettings, String> {
    Ok(VllmSettings {
        max_model_len: vllm.max_model_len.unwrap_or(65_536),
        max_num_seqs: vllm.max_num_seqs.unwrap_or(8),
        gpu_memory_utilization: vllm.gpu_memory_utilization.unwrap_or(0.9),
        thinking: match vllm.thinking.as_deref() {
            None | Some("off") => Thinking::Off,
            Some("on") => Thinking::On,
            Some(_) => return Err("fixture:unknown-thinking".to_owned()),
        },
        reasoning_effort: vllm.reasoning_effort.clone(),
        sampling: vllm.sampling.clone(),
        extra_args: vllm
            .extra_args
            .clone()
            .unwrap_or_else(|| vec!["--kv-cache-dtype".to_owned(), "fp8".to_owned()]),
    })
}

fn build_model(input: &ModelInput) -> Result<RunpodModel, String> {
    let profiles = input.profiles.as_deref().unwrap_or_default();
    let (profile, gpu_types, first_vllm) = match profiles.first() {
        Some(first) => (
            first.name.clone(),
            Some(first.gpu_types.clone()),
            build_vllm(&first.vllm)?,
        ),
        None => (
            "default".to_owned(),
            input.gpu_types.clone(),
            build_vllm(&input.vllm)?,
        ),
    };
    let fallbacks = profiles
        .iter()
        .skip(1)
        .map(|profile| {
            Ok(RunpodProfile {
                name: profile.name.clone(),
                gpu_types: profile.gpu_types.clone(),
                vllm: build_vllm(&profile.vllm)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(RunpodModel {
        hf_model: input
            .hf_model
            .clone()
            .unwrap_or_else(|| "Qwen/Qwen3.8-27B-FP8".to_owned()),
        image: ident(input.image.as_deref().unwrap_or("vllm/vllm-openai:v0.27.1"))?,
        gpu_types: gpu_types
            .unwrap_or_else(|| vec!["NVIDIA L40S".to_owned(), "NVIDIA H100 NVL".to_owned()]),
        cloud_type: match input.cloud_type.as_deref() {
            None | Some("SECURE") => CloudType::Secure,
            Some("COMMUNITY") => CloudType::Community,
            Some(_) => return Err("fixture:unknown-cloud-type".to_owned()),
        },
        container_disk_gb: input.container_disk_gb.unwrap_or(80),
        cache: input.cache.as_ref().map(|cache| NetworkVolume {
            volume_id: cache.volume_id.clone(),
            mount_path: cache.mount_path.clone(),
        }),
        data_center_ids: input.data_center_ids.clone().unwrap_or_default(),
        vllm: first_vllm,
        api_key_secret: input
            .api_key_secret
            .clone()
            .unwrap_or_else(|| "qwen_vllm_key".to_owned()),
        startup_deadline_ms: input.startup_deadline_ms.unwrap_or(600_000),
        idle_timeout_ms: input.idle_timeout_ms.unwrap_or(1_800_000),
        crash_restart_limit: input.crash_restart_limit.unwrap_or(2),
        crash_window_ms: input.crash_window_ms.unwrap_or(600_000),
        profile,
        fallbacks,
    })
}

fn build_models(
    models: &BTreeMap<String, ModelInput>,
) -> Result<BTreeMap<Identifier, RunpodModel>, String> {
    models
        .iter()
        .map(|(alias, model)| Ok((ident(alias)?, build_model(model)?)))
        .collect()
}

fn policy(max_active: u32) -> Result<HostingPolicy, String> {
    Ok(HostingPolicy {
        controller: ident("controller-a")?,
        provider: ident("runpod")?,
        account: ident("account-1")?,
        ledger: ident("ledger-1")?,
        max_active,
        max_lifetime_ms: 86_400_000,
        lease_ms: 3_600_000,
    })
}

fn refusal(error: PoolError) -> String {
    match error {
        PoolError::InvalidModel(config) => format!("invalid-model:{}", config.code()),
        PoolError::Hosting(hosting) => format!("hosting:{}", hosting.code()),
        PoolError::SpendCeiling(SpendRefusal::Over {
            ceiling,
            over_micro_usd,
        }) => format!(
            "spend-ceiling:{}:over_usd={}",
            ceiling.code(),
            llm_runpod::usd(over_micro_usd)
        ),
        PoolError::SpendCeiling(SpendRefusal::Unknown) => "spend-ceiling:unknown".to_owned(),
        other => other.code().to_owned(),
    }
}

fn status_code(status: PodStatus) -> &'static str {
    match status {
        PodStatus::Created => "created",
        PodStatus::Running => "running",
        PodStatus::Exited => "exited",
        PodStatus::Terminated => "terminated",
    }
}

fn joined(ids: &[Identifier]) -> String {
    ids.iter()
        .map(Identifier::as_str)
        .collect::<Vec<_>>()
        .join(",")
}

fn cleanup(report: &CleanupReport) -> String {
    format!(
        "idle=[{}] retired=[{}] replaced=[{}] orphans=[{}]",
        joined(&report.idle),
        joined(&report.retired),
        joined(&report.replaced),
        joined(&report.orphans)
    )
}

/// A `wake` or `status` answer: `<word> phase=<..> sub_phase=<..|none> profile=<..|none>`.
fn status_fact(word: &str, status: &ModelStatus) -> String {
    format!(
        "{word} phase={} sub_phase={} profile={}",
        status.phase.code(),
        status.sub_phase.unwrap_or("none"),
        status.profile.as_deref().unwrap_or("none")
    )
}

fn lease_fact(lease: &StreamLease) -> String {
    format!(
        "{} {}",
        lease.endpoint().unwrap_or("none"),
        lease
            .served_model()
            .map_or("unreported", Identifier::as_str)
    )
}

struct Run {
    fixture: Fixture,
    clock: ManualClock,
    registry: Arc<LeaseRegistry>,
    policy: HostingPolicy,
    models: BTreeMap<Identifier, RunpodModel>,
    spend: SpendPolicy,
    pool: Arc<RunpodPool<Fixture>>,
    /// Pools a restart replaced. They stay alive, as a crashed process's claims would.
    replaced: Vec<Arc<RunpodPool<Fixture>>>,
    held: Vec<StreamLease>,
    leases: Vec<String>,
    /// The profile each ready lease names, `unknown` when it names none.
    lease_profiles: Vec<String>,
    cleanups: Vec<String>,
}

impl Run {
    fn authorization() -> Result<ComputeAuthorization, String> {
        Ok(ComputeAuthorization {
            ledger: ident("ledger-1")?,
            reservation: ident("reservation-1")?,
        })
    }

    fn ensure(&mut self, alias: &str, attempts: Option<usize>, keep: bool) -> String {
        let (Ok(alias), Ok(authorization)) = (ident(alias), Self::authorization()) else {
            return "err:fixture:invalid-identifier".to_owned();
        };
        let limit = attempts.unwrap_or(20).min(MAX_ATTEMPTS);
        let retry = attempts.is_some() || keep;
        for _ in 0..limit.max(1) {
            match self.pool.ensure(&alias, &authorization) {
                Ok(lease) => {
                    self.leases.push(lease_fact(&lease));
                    self.lease_profiles
                        .push(lease.profile().unwrap_or("unknown").to_owned());
                    if keep {
                        self.held.push(lease);
                    }
                    return "ready".to_owned();
                }
                Err(PoolError::Starting) if retry => self.clock.advance(RETRY_STEP_MS),
                Err(error) => return format!("err:{}", refusal(error)),
            }
        }
        "never-ready".to_owned()
    }

    fn wake(&self, alias: &str) -> String {
        let (Ok(alias), Ok(authorization)) = (ident(alias), Self::authorization()) else {
            return "err:fixture:invalid-identifier".to_owned();
        };
        match self.pool.wake(&alias, &authorization) {
            Ok(status) => status_fact("woke", &status),
            Err(error) => format!("err:{}", refusal(error)),
        }
    }

    fn status(&self, alias: &str) -> String {
        let Ok(alias) = ident(alias) else {
            return "err:fixture:invalid-identifier".to_owned();
        };
        match self.pool.status(&alias) {
            Ok(status) => status_fact("status", &status),
            Err(error) => format!("err:{}", refusal(error)),
        }
    }

    fn concurrent(&self, alias: &str, callers: usize) -> String {
        let (Ok(alias), Ok(authorization)) = (ident(alias), Self::authorization()) else {
            return "err:fixture:invalid-identifier".to_owned();
        };
        let callers = callers.clamp(1, MAX_CALLERS);
        let barrier = Arc::new(Barrier::new(callers));
        let handles: Vec<_> = (0..callers)
            .map(|_| {
                let pool = Arc::clone(&self.pool);
                let barrier = Arc::clone(&barrier);
                let alias = alias.clone();
                let authorization = authorization.clone();
                thread::spawn(move || {
                    barrier.wait();
                    pool.ensure(&alias, &authorization).map(drop)
                })
            })
            .collect();
        let (mut served, mut starting, mut other) = (0_usize, 0_usize, 0_usize);
        for answer in handles.into_iter().map(thread::JoinHandle::join) {
            match answer {
                Ok(Ok(())) => served += 1,
                Ok(Err(PoolError::Starting)) => starting += 1,
                _ => other += 1,
            }
        }
        format!("concurrent:served={served} starting={starting} other={other}")
    }

    fn restart(&mut self, models: Option<&BTreeMap<String, ModelInput>>) -> String {
        let models = match models.map(build_models) {
            None => self.models.clone(),
            Some(Ok(models)) => models,
            Some(Err(code)) => return format!("err:{code}"),
        };
        match RunpodPool::restore(
            self.policy.clone(),
            Arc::clone(&self.registry),
            self.fixture.clone(),
            models.clone(),
            self.pool.snapshot(),
            Arc::new(self.clock.clone()),
        ) {
            Ok(pool) => {
                let pool = pool.with_spend(self.spend.clone());
                let previous = std::mem::replace(&mut self.pool, Arc::new(pool));
                self.replaced.push(previous);
                self.models = models;
                "ok".to_owned()
            }
            Err(error) => format!("err:{}", refusal(error)),
        }
    }

    fn insert(
        &self,
        name: &str,
        env: &BTreeMap<String, String>,
        request_of: Option<&str>,
    ) -> String {
        let mut env = env.clone();
        if let Some(deployment) = request_of {
            let Some(record) = self
                .pool
                .view()
                .deployments
                .into_iter()
                .find(|record| record.deployment.as_str() == deployment)
            else {
                return "err:fixture:no-such-record".to_owned();
            };
            env.insert(
                TAG_REQUEST.to_owned(),
                record.request_id.as_str().to_owned(),
            );
        }
        format!("inserted:{}", self.fixture.runpod.insert_pod(name, env))
    }

    fn step(&mut self, step: &Step) -> String {
        let runpod = &self.fixture.runpod;
        match step {
            Step::Ensure { alias } => return self.ensure(alias, None, false),
            Step::UntilReady {
                alias,
                max_attempts,
            } => return self.ensure(alias, Some(max_attempts.unwrap_or(20)), false),
            Step::Hold {
                alias,
                max_attempts,
            } => return self.ensure(alias, Some(max_attempts.unwrap_or(20)), true),
            Step::Release => self.held.clear(),
            Step::Wake { alias } => return self.wake(alias),
            Step::Status { alias } => return self.status(alias),
            Step::Concurrent { alias, callers } => return self.concurrent(alias, *callers),
            Step::Advance { ms } => self.clock.advance(*ms),
            Step::Reap => match self.pool.reap() {
                Ok(report) => self.cleanups.push(cleanup(&report)),
                Err(error) => return format!("err:{}", refusal(error)),
            },
            Step::Restart { models } => return self.restart(models.as_ref()),
            Step::RefuseGpu { gpu } => runpod.refuse_gpu(gpu),
            Step::NoCapacityGpu { gpu } => runpod.no_capacity_gpu(gpu),
            Step::LoseNextCreate { allocates } => runpod.lose_next_create(*allocates),
            Step::CreateTakes { ms } => self.fixture.create_ms.store(*ms, Ordering::SeqCst),
            Step::ReadyAfter { probes } => runpod.ready_after(*probes),
            Step::Uptimes { script } => runpod.uptimes(script.clone()),
            Step::RefuseCredential { pod } => runpod.refuse_credential(pod),
            Step::PartialListing { partial } => runpod.partial_listing(*partial),
            Step::ProbeUnreachable { unreachable } => runpod.probe_unreachable(*unreachable),
            Step::AccountSpend {
                daily_usd,
                total_usd,
            } => match (amount(*daily_usd), amount(*total_usd)) {
                (Ok(daily_micro_usd), Ok(total_micro_usd)) => {
                    runpod.set_account_spend(Some(AccountSpend {
                        daily_micro_usd,
                        total_micro_usd,
                    }));
                }
                (Err(code), _) | (_, Err(code)) => return format!("err:{code}"),
            },
            Step::Billed { entries } => {
                let mut billed = Vec::with_capacity(entries.len());
                for entry in entries {
                    match amount(entry.usd) {
                        Ok(micro_usd) => billed.push((entry.at_ms, micro_usd)),
                        Err(code) => return format!("err:{code}"),
                    }
                }
                runpod.set_billed(billed);
            }
            Step::SetStatus { pod, status } => {
                let status = match status.as_str() {
                    "Created" => PodStatus::Created,
                    "Running" => PodStatus::Running,
                    "Exited" => PodStatus::Exited,
                    "Terminated" => PodStatus::Terminated,
                    _ => return "err:fixture:unknown-status".to_owned(),
                };
                runpod.set_status(pod, status);
            }
            Step::Vanish { pod } => runpod.vanish(pod),
            Step::Retag { pod, owner, epoch } => runpod.retag(pod, owner, *epoch),
            Step::InsertPod {
                name,
                env,
                request_of,
            } => return self.insert(name, env, request_of.as_deref()),
            Step::Wedge { pod } => {
                self.fixture
                    .wedged
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(pod.clone());
            }
        }
        "ok".to_owned()
    }
}

fn emulator_facts(facts: &mut Value, runpod: &EmulatedRunpod) {
    let requests = runpod.requests();
    facts["create_calls"] = json!(runpod.create_calls());
    facts["lists"] = json!(runpod.lists());
    facts["spend_reads"] = json!(runpod.spend_reads());
    facts["requested_gpus"] = json!(
        requests
            .iter()
            .map(|request| request.gpu_type.clone())
            .collect::<Vec<_>>()
    );
    facts["terminations"] = json!(runpod.terminations());
    facts["pods"] = json!(
        runpod
            .pods()
            .iter()
            .map(|pod| format!("{}:{}:{}", pod.id, pod.name, status_code(pod.status)))
            .collect::<Vec<_>>()
    );
}

fn request_facts(facts: &mut Value, request: &PodRequest, recorded: &BTreeSet<String>) {
    facts["last_request"] = json!(format!(
        "name={} image={} gpu={} count={} cloud={} disk={} ports={} interruptible={} volume={} mount={} dcs={}",
        request.name,
        request.image,
        request.gpu_type,
        request.gpu_count,
        request.cloud_type.as_str(),
        request.container_disk_gb,
        request.ports.join(","),
        request.interruptible,
        request.network_volume_id.as_deref().unwrap_or("none"),
        request.volume_mount_path.as_deref().unwrap_or("none"),
        request.data_center_ids.join(",")
    ));
    facts["last_entrypoint"] = json!(request.docker_entrypoint);
    facts["last_env"] = json!(
        request
            .env
            .iter()
            .map(|(key, value)| {
                if key == TAG_REQUEST {
                    let recorded = if recorded.contains(value) {
                        "recorded"
                    } else {
                        "unrecorded"
                    };
                    format!("{key}={recorded}")
                } else {
                    format!("{key}={value}")
                }
            })
            .collect::<Vec<_>>()
    );
}

fn pool_facts(facts: &mut Value, run: &Run) {
    let view = run.pool.view();
    facts["phases"] = json!(
        view.deployments
            .iter()
            .map(|record| format!("{}:{}", record.deployment, record.phase.code()))
            .collect::<Vec<_>>()
    );
    facts["observed"] = json!(
        view.deployments
            .iter()
            .map(|record| match &record.observed {
                None => format!("{} unobserved", record.deployment),
                Some(observed) => format!(
                    "{} ready={} served={} endpoint={}",
                    record.deployment,
                    observed.ready.map_or("unreported", |ready| if ready {
                        "true"
                    } else {
                        "false"
                    }),
                    observed
                        .served_model
                        .as_ref()
                        .map_or("unreported", Identifier::as_str),
                    observed.endpoint.as_deref().unwrap_or("unreported")
                ),
            })
            .collect::<Vec<_>>()
    );
    facts["stop_required"] = json!(
        view.totals
            .stop_required
            .iter()
            .map(Identifier::as_str)
            .collect::<Vec<_>>()
    );
    facts["transferred"] = json!(
        view.totals
            .transferred
            .iter()
            .map(Identifier::as_str)
            .collect::<Vec<_>>()
    );
    let recorded: BTreeSet<String> = view
        .deployments
        .iter()
        .map(|record| record.request_id.as_str().to_owned())
        .collect();
    if let Some(request) = run.fixture.runpod.requests().last() {
        request_facts(facts, request, &recorded);
    }
}

fn run(program_json: &str) -> Value {
    let mut facts = json!({
        "valid_program": false, "error_code": null, "results": [], "create_calls": null,
        "lists": null, "spend_reads": null, "requested_gpus": [], "terminations": [], "pods": [], "leases": [], "lease_profiles": [],
        "cleanups": [], "phases": [], "observed": [], "stop_required": [], "transferred": [],
        "last_request": null, "last_entrypoint": [], "last_env": []
    });
    if program_json.len() > MAX_PROGRAM_BYTES {
        return facts;
    }
    let Ok(program) = serde_json::from_str::<Program>(program_json) else {
        return facts;
    };
    if program.steps.len() > MAX_STEPS {
        return facts;
    }
    facts["valid_program"] = json!(true);
    let setup = build_models(&program.models).and_then(|models| {
        Ok((
            models,
            policy(program.max_active.unwrap_or(16))?,
            spend_policy(program.spend.as_ref(), &program.models)?,
        ))
    });
    let (models, policy, spend) = match setup {
        Ok(setup) => setup,
        Err(code) => {
            facts["error_code"] = json!(code);
            return facts;
        }
    };
    let clock = ManualClock::new(1_000);
    let fixture = Fixture::new(clock.clone());
    let registry = Arc::new(LeaseRegistry::default());
    let pool = match RunpodPool::new(
        policy.clone(),
        Arc::clone(&registry),
        fixture.clone(),
        models.clone(),
        Arc::new(clock.clone()),
    ) {
        Ok(pool) => pool.with_spend(spend.clone()),
        Err(error) => {
            facts["error_code"] = json!(refusal(error));
            emulator_facts(&mut facts, &fixture.runpod);
            return facts;
        }
    };
    let mut run = Run {
        fixture,
        clock,
        registry,
        policy,
        models,
        spend,
        pool: Arc::new(pool),
        replaced: Vec::new(),
        held: Vec::new(),
        leases: Vec::new(),
        lease_profiles: Vec::new(),
        cleanups: Vec::new(),
    };
    let results: Vec<String> = program.steps.iter().map(|step| run.step(step)).collect();
    facts["results"] = json!(results);
    facts["leases"] = json!(run.leases);
    facts["lease_profiles"] = json!(run.lease_profiles);
    facts["cleanups"] = json!(run.cleanups);
    emulator_facts(&mut facts, &run.fixture.runpod);
    pool_facts(&mut facts, &run);
    run.held.clear();
    run.replaced.clear();
    facts
}
