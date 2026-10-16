//! Model profiles (story:model-profile-fallback, `spec/domains/runpod.yaml` `ModelProfile`): a
//! model declares an ordered list of profiles, and a start whose every create of a profile was
//! refused for capacity moves on to the next profile, inside the same start. Any other refusal,
//! a lost create answer and a spend refusal end the start. The conformance scenarios cover the
//! fallback and the non-capacity refusal; these cover the rest: the lost answer, the spend
//! check before each profile's creates, the one startup deadline, the profile a lease and an
//! adopted pod report, and the declaration's own refusals. Every test drives `EmulatedRunpod`.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, ConfigError, CreateAnswer, EmulatedRunpod,
    HostingPolicy, Identifier, LeaseRegistry, ManualClock, PodListing, PodRequest, PoolError,
    Probe, ProbeTarget, RunpodModel, RunpodPool, RunpodProfile, RunpodTransport, SpendPolicy,
    StreamLease, TerminateAnswer, Thinking, VllmSettings,
};

const ALIAS: &str = "qwen";

fn id(value: &str) -> Identifier {
    Identifier::new(value).expect("test identifier")
}

fn policy() -> HostingPolicy {
    HostingPolicy {
        controller: id("controller-a"),
        provider: id("runpod"),
        account: id("account-1"),
        ledger: id("ledger-1"),
        max_active: 16,
        max_lifetime_ms: 86_400_000,
        lease_ms: 3_600_000,
    }
}

fn authorization() -> ComputeAuthorization {
    ComputeAuthorization {
        ledger: id("ledger-1"),
        reservation: id("reservation-1"),
    }
}

fn vllm(max_model_len: u32, max_num_seqs: u32, extra_args: &[&str]) -> VllmSettings {
    VllmSettings {
        max_model_len,
        max_num_seqs,
        gpu_memory_utilization: 0.9,
        thinking: Thinking::Off,
        reasoning_effort: None,
        sampling: None,
        extra_args: extra_args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
}

/// Three profiles: `l40s-32k` (L40S, A40), `h100-64k` (H100 NVL, MTP) and `b200-128k` (B200).
fn model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3.8-27B-FP8".to_owned(),
        image: id("vllm/vllm-openai:v0.27.1"),
        gpu_types: vec!["NVIDIA L40S".to_owned(), "NVIDIA A40".to_owned()],
        cloud_type: CloudType::Secure,
        container_disk_gb: 80,
        cache: None,
        data_center_ids: Vec::new(),
        vllm: vllm(32_768, 8, &[]),
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 86_400_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "l40s-32k".to_owned(),
        fallbacks: vec![
            RunpodProfile {
                name: "h100-64k".to_owned(),
                gpu_types: vec!["NVIDIA H100 NVL".to_owned()],
                vllm: vllm(65_536, 16, &["--speculative-config", r#"{"method":"mtp"}"#]),
            },
            RunpodProfile {
                name: "b200-128k".to_owned(),
                gpu_types: vec!["NVIDIA B200".to_owned()],
                vllm: vllm(131_072, 32, &[]),
            },
        ],
    }
}

fn models(model: RunpodModel) -> BTreeMap<Identifier, RunpodModel> {
    BTreeMap::from([(id(ALIAS), model)])
}

/// A transport over the emulator whose spend reads answer a script, one entry per read; a read
/// past the script's end answers nothing.
#[derive(Clone)]
struct ScriptedSpend {
    runpod: EmulatedRunpod,
    answers: Arc<Mutex<VecDeque<Option<AccountSpend>>>>,
}

impl RunpodTransport for ScriptedSpend {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.runpod.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.runpod.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.runpod.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        self.runpod.probe_ready(target)
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.runpod.container_started_at(pod_id)
    }
    fn account_spend(&mut self, _day_start_ms: u64) -> Option<AccountSpend> {
        self.answers
            .lock()
            .expect("script")
            .pop_front()
            .unwrap_or(None)
    }
}

struct Stack<T: RunpodTransport> {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    leases: Arc<LeaseRegistry>,
    pool: RunpodPool<T>,
}

fn stack() -> Stack<EmulatedRunpod> {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let leases = Arc::new(LeaseRegistry::default());
    let pool = RunpodPool::new(
        policy(),
        Arc::clone(&leases),
        runpod.clone(),
        models(model()),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    Stack {
        runpod,
        clock,
        leases,
        pool,
    }
}

/// `ensure` until a lease is handed out, at most five times, the clock standing still.
fn ready<T: RunpodTransport>(pool: &RunpodPool<T>) -> Result<StreamLease, PoolError> {
    let mut last = Err(PoolError::Starting);
    for _ in 0..5 {
        last = pool.ensure(&id(ALIAS), &authorization());
        if !matches!(last, Err(PoolError::Starting)) {
            break;
        }
    }
    last
}

fn requested_gpus(runpod: &EmulatedRunpod) -> Vec<String> {
    runpod
        .requests()
        .iter()
        .map(|request| request.gpu_type.clone())
        .collect()
}

/// The value after `flag` in the last create's vLLM entrypoint.
fn last_arg(runpod: &EmulatedRunpod, flag: &str) -> Option<String> {
    let request = runpod.requests().pop()?;
    let args = request.docker_entrypoint;
    let at = args.iter().position(|arg| arg == flag)?;
    args.get(at + 1).cloned()
}

// ---- Acceptance 2: a capacity refusal moves on to the next profile ----

#[test]
fn a_profile_out_of_capacity_starts_the_next_profile_with_its_own_vllm_settings() {
    let stack = stack();
    stack.runpod.no_capacity_gpu("NVIDIA L40S");
    stack.runpod.no_capacity_gpu("NVIDIA A40");
    let lease = ready(&stack.pool).expect("the second profile serves");
    assert_eq!(
        requested_gpus(&stack.runpod),
        ["NVIDIA L40S", "NVIDIA A40", "NVIDIA H100 NVL"]
    );
    assert_eq!(lease.profile(), Some("h100-64k"));
    assert_eq!(
        last_arg(&stack.runpod, "--max-model-len").as_deref(),
        Some("65536")
    );
    assert_eq!(
        last_arg(&stack.runpod, "--max-num-seqs").as_deref(),
        Some("16")
    );
    assert_eq!(
        last_arg(&stack.runpod, "--speculative-config").as_deref(),
        Some(r#"{"method":"mtp"}"#)
    );
}

#[test]
fn every_profile_out_of_capacity_is_no_capacity_and_leaves_no_pod() {
    let stack = stack();
    for gpu in [
        "NVIDIA L40S",
        "NVIDIA A40",
        "NVIDIA H100 NVL",
        "NVIDIA B200",
    ] {
        stack.runpod.no_capacity_gpu(gpu);
    }
    assert_eq!(ready(&stack.pool).err(), Some(PoolError::NoCapacity));
    assert_eq!(stack.runpod.create_calls(), 4);
    assert!(stack.runpod.pods().is_empty());
}

#[test]
fn the_first_profile_that_has_capacity_serves_and_later_profiles_are_not_tried() {
    let stack = stack();
    let lease = ready(&stack.pool).expect("the first profile serves");
    assert_eq!(lease.profile(), Some("l40s-32k"));
    assert_eq!(requested_gpus(&stack.runpod), ["NVIDIA L40S"]);
    assert_eq!(
        last_arg(&stack.runpod, "--max-model-len").as_deref(),
        Some("32768")
    );
}

/// A profile with one create refused for anything but capacity ends the start, even when its
/// other GPU type answered "no capacity".
#[test]
fn a_non_capacity_refusal_in_a_profile_is_not_retried_on_the_next_profile() {
    let stack = stack();
    stack.runpod.no_capacity_gpu("NVIDIA L40S");
    stack.runpod.refuse_gpu("NVIDIA A40");
    assert_eq!(ready(&stack.pool).err(), Some(PoolError::NoCapacity));
    assert_eq!(requested_gpus(&stack.runpod), ["NVIDIA L40S", "NVIDIA A40"]);
    assert!(stack.runpod.pods().is_empty());
}

#[test]
fn a_lost_create_answer_is_not_retried_on_the_next_profile() {
    let stack = stack();
    stack.runpod.no_capacity_gpu("NVIDIA L40S");
    stack.runpod.lose_next_create(false);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(requested_gpus(&stack.runpod), ["NVIDIA L40S", "NVIDIA A40"]);
    assert_eq!(
        stack.runpod.create_calls(),
        2,
        "no H100 create after a lost answer"
    );
}

/// The spend check runs before each create of each profile, and a spend refusal on the second
/// profile neither creates nor moves on to the third.
#[test]
fn the_spend_check_runs_before_each_profiles_creates_and_a_refusal_ends_the_start() {
    let runpod = EmulatedRunpod::new();
    runpod.no_capacity_gpu("NVIDIA L40S");
    runpod.no_capacity_gpu("NVIDIA A40");
    let under = Some(AccountSpend {
        daily_micro_usd: 0,
        total_micro_usd: 0,
    });
    let over = Some(AccountSpend {
        daily_micro_usd: 100_000_000,
        total_micro_usd: 100_000_000,
    });
    let answers = Arc::new(Mutex::new(VecDeque::from([under, under, over])));
    let transport = ScriptedSpend {
        runpod: runpod.clone(),
        answers: Arc::clone(&answers),
    };
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport,
        models(model()),
        Arc::new(clock),
    )
    .expect("pool")
    .with_spend(SpendPolicy {
        daily_cap_micro_usd: Some(10_000_000),
        total_cap_micro_usd: None,
        price_per_hour_micro_usd: BTreeMap::from([(id(ALIAS), 2_000_000)]),
    });
    assert!(matches!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::SpendCeiling(_))
    ));
    assert_eq!(
        requested_gpus(&runpod),
        ["NVIDIA L40S", "NVIDIA A40"],
        "the H100 create was refused before it was sent"
    );
    assert!(
        answers.lock().expect("script").is_empty(),
        "three spend reads: one before each of three creates"
    );
}

/// One startup deadline covers the whole start: a pod of the second profile is held to the
/// deadline counted from the start, not from its own create.
#[test]
fn one_startup_deadline_covers_every_profile_of_a_start() {
    let stack = stack();
    stack.runpod.no_capacity_gpu("NVIDIA L40S");
    stack.runpod.no_capacity_gpu("NVIDIA A40");
    stack.runpod.ready_after(u32::MAX);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.clock.advance(600_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.clock.advance(1);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::StartupDeadline)
    );
    assert_eq!(stack.runpod.terminations(), ["pod1"]);
}

// ---- Acceptance 3 (first half): the profile that served ----

/// A restarted pool adopts the pod and still knows its profile, from the pod's GPU type.
#[test]
fn an_adopted_pod_reports_the_profile_its_gpu_type_belongs_to() {
    let stack = stack();
    stack.runpod.no_capacity_gpu("NVIDIA L40S");
    stack.runpod.no_capacity_gpu("NVIDIA A40");
    drop(ready(&stack.pool).expect("ready"));
    let restored = RunpodPool::restore(
        policy(),
        Arc::clone(&stack.leases),
        stack.runpod.clone(),
        models(model()),
        stack.pool.snapshot(),
        Arc::new(stack.clock.clone()),
    )
    .expect("restore");
    let lease = ready(&restored).expect("adopted");
    assert_eq!(stack.runpod.create_calls(), 3, "adopted, not created again");
    assert_eq!(lease.profile(), Some("h100-64k"));
}

/// The existing single declaration is one profile named `default`, and starts as it did.
#[test]
fn a_model_without_fallbacks_is_one_profile() {
    let runpod = EmulatedRunpod::new();
    let mut single = model();
    single.profile = "default".to_owned();
    single.fallbacks = Vec::new();
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        models(single),
        Arc::new(ManualClock::new(1_000)),
    )
    .expect("pool");
    runpod.no_capacity_gpu("NVIDIA L40S");
    let lease = ready(&pool).expect("the second GPU of the one profile serves");
    assert_eq!(lease.profile(), Some("default"));
    assert_eq!(requested_gpus(&runpod), ["NVIDIA L40S", "NVIDIA A40"]);
}

// ---- Acceptance 1: the declaration ----

#[test]
fn a_gpu_type_in_two_profiles_or_a_repeated_name_is_a_profile_conflict() {
    let mut shared_gpu = model();
    shared_gpu.fallbacks[1]
        .gpu_types
        .push("NVIDIA A40".to_owned());
    assert_eq!(shared_gpu.validate(), Err(ConfigError::ProfileConflict));
    let mut repeated_name = model();
    repeated_name.fallbacks[1].name = "l40s-32k".to_owned();
    assert_eq!(repeated_name.validate(), Err(ConfigError::ProfileConflict));
    assert_eq!(ConfigError::ProfileConflict.code(), "profile-conflict");
    assert!(ConfigError::ALL.contains(&ConfigError::ProfileConflict));
    assert_eq!(model().validate(), Ok(()));
}

#[test]
fn every_fallback_profile_is_checked_as_the_model_is() {
    let mut no_gpu = model();
    no_gpu.fallbacks[0].gpu_types.clear();
    assert_eq!(no_gpu.validate(), Err(ConfigError::NoGpu));
    let mut zero = model();
    zero.fallbacks[1].vllm.max_model_len = 0;
    assert_eq!(zero.validate(), Err(ConfigError::ZeroSetting));
    let mut utilization = model();
    utilization.fallbacks[0].vllm.gpu_memory_utilization = 1.5;
    assert_eq!(utilization.validate(), Err(ConfigError::GpuUtilization));
    let mut unsafe_arg = model();
    unsafe_arg.fallbacks[0]
        .vllm
        .extra_args
        .push("a\nb".to_owned());
    assert_eq!(unsafe_arg.validate(), Err(ConfigError::UnsafeText));
    let mut bad_name = model();
    bad_name.fallbacks[0].name = "h100 64k".to_owned();
    assert_eq!(bad_name.validate(), Err(ConfigError::UnsafeText));
    let mut empty_first = model();
    empty_first.profile = String::new();
    assert_eq!(empty_first.validate(), Err(ConfigError::UnsafeText));
}

#[test]
fn an_invalid_profile_refuses_the_pool() {
    let mut shared_gpu = model();
    shared_gpu.fallbacks[0].gpu_types = vec!["NVIDIA L40S".to_owned()];
    let refused = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        EmulatedRunpod::new(),
        models(shared_gpu),
        Arc::new(ManualClock::new(1_000)),
    )
    .err();
    assert_eq!(
        refused,
        Some(PoolError::InvalidModel(ConfigError::ProfileConflict))
    );
}
