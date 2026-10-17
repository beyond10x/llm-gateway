//! The pool's two calls behind the owner's lifecycle routes (story:model-wake-and-status,
//! `spec/domains/runpod.yaml` `Pool`): `status` reads the controller's records and contacts
//! nobody, and `wake` is one `ensure` followed by `status`. The scenarios under
//! `contracts/runpod/scenarios` (`a-wake-*`, `status-*`) cover the phases, the spend check and
//! the profile fallback; these cover what a scenario cannot count: no transport call of any
//! kind from a status, the idle window a wake renews and a status does not, what a restarted
//! pool reports before it observes, and the phase codes against the specification. Every test
//! drives `EmulatedRunpod`.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, CreateAnswer, EmulatedRunpod, HostingPolicy,
    Identifier, LeaseRegistry, ManualClock, ModelPhase, PodListing, PodRequest, PoolError, Probe,
    ProbeTarget, RunpodModel, RunpodPool, RunpodTransport, TerminateAnswer, Thinking, VllmSettings,
};

const SPEC: &str = include_str!("../../../spec/domains/gateway.yaml");
const IDLE_MS: u64 = 60_000;

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

fn model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3.8-27B-FP8".to_owned(),
        image: id("vllm/vllm-openai:v0.27.1"),
        gpu_types: vec!["NVIDIA L40S".to_owned()],
        cloud_type: CloudType::Secure,
        container_disk_gb: 80,
        cache: None,
        data_center_ids: Vec::new(),
        vllm: VllmSettings {
            max_model_len: 32_768,
            max_num_seqs: 8,
            gpu_memory_utilization: 0.9,
            thinking: Thinking::Off,
            reasoning_effort: None,
            sampling: None,
            extra_args: Vec::new(),
        },
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: IDLE_MS,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "l40s-32k".to_owned(),
        fallbacks: Vec::new(),
    }
}

fn models() -> BTreeMap<Identifier, RunpodModel> {
    BTreeMap::from([(id("qwen"), model())])
}

/// The emulator, counting every call the pool makes of its transport.
#[derive(Clone, Default)]
struct Counted {
    runpod: EmulatedRunpod,
    calls: Arc<AtomicUsize>,
}

impl Counted {
    fn count(&self) {
        self.calls.fetch_add(1, Ordering::SeqCst);
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl RunpodTransport for Counted {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.count();
        self.runpod.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.count();
        self.runpod.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.count();
        self.runpod.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        self.count();
        self.runpod.probe_ready(target)
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.count();
        self.runpod.container_started_at(pod_id)
    }
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        self.count();
        self.runpod.account_spend(day_start_ms)
    }
}

fn pool(transport: Counted, clock: &ManualClock) -> RunpodPool<Counted> {
    RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport,
        models(),
        Arc::new(clock.clone()),
    )
    .unwrap()
}

#[test]
fn a_status_makes_no_transport_call_in_any_phase() {
    let transport = Counted::default();
    let clock = ManualClock::new(1_000);
    let pool = pool(transport.clone(), &clock);
    let qwen = id("qwen");
    let mut phases = Vec::new();
    let mut observe = |pool: &RunpodPool<Counted>| {
        let before = transport.calls();
        let status = pool.status(&qwen).unwrap();
        assert_eq!(transport.calls(), before, "a status called the transport");
        phases.push(status.phase);
    };
    observe(&pool);
    transport.runpod.ready_after(1);
    assert_eq!(
        pool.wake(&qwen, &authorization()).unwrap().phase,
        ModelPhase::Starting
    );
    observe(&pool);
    assert_eq!(
        pool.wake(&qwen, &authorization()).unwrap().phase,
        ModelPhase::Starting
    );
    observe(&pool);
    assert_eq!(
        pool.wake(&qwen, &authorization()).unwrap().phase,
        ModelPhase::Ready
    );
    observe(&pool);
    clock.advance(IDLE_MS);
    let reaped = pool.reap().unwrap();
    assert_eq!(reaped.idle.len(), 1, "{reaped:?}");
    observe(&pool);
    assert_eq!(
        phases,
        [
            ModelPhase::Stopped,
            ModelPhase::Starting,
            ModelPhase::Starting,
            ModelPhase::Ready,
            ModelPhase::Stopped,
        ]
    );
    assert_eq!(
        pool.status(&id("absent")).unwrap_err(),
        PoolError::UnknownModel
    );
    assert_eq!(transport.runpod.create_calls(), 1);
}

#[test]
fn a_wake_of_a_ready_pod_renews_its_idle_window_and_a_status_does_not() {
    let transport = Counted::default();
    let clock = ManualClock::new(1_000);
    let pool = pool(transport.clone(), &clock);
    let qwen = id("qwen");
    pool.wake(&qwen, &authorization()).unwrap();
    assert_eq!(
        pool.wake(&qwen, &authorization()).unwrap().phase,
        ModelPhase::Ready
    );
    clock.advance(IDLE_MS - 1);
    // A use: the idle window starts again from here.
    assert_eq!(
        pool.wake(&qwen, &authorization()).unwrap().phase,
        ModelPhase::Ready
    );
    clock.advance(IDLE_MS - 1);
    assert!(
        pool.reap().unwrap().idle.is_empty(),
        "a wake did not count as a use"
    );
    // A status is not a use: the window passes under it.
    assert_eq!(pool.status(&qwen).unwrap().phase, ModelPhase::Ready);
    clock.advance(1);
    assert_eq!(pool.reap().unwrap().idle, [id("qwen-1")]);
    assert_eq!(transport.runpod.create_calls(), 1);
}

#[test]
fn a_restarted_pool_reports_a_pod_it_has_not_observed_yet_as_starting_with_no_sub_phase() {
    let transport = Counted::default();
    let clock = ManualClock::new(1_000);
    let registry = Arc::new(LeaseRegistry::default());
    let first = RunpodPool::new(
        policy(),
        Arc::clone(&registry),
        transport.clone(),
        models(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    let qwen = id("qwen");
    first.wake(&qwen, &authorization()).unwrap();
    let ready = first.wake(&qwen, &authorization()).unwrap();
    assert_eq!(ready.phase, ModelPhase::Ready);
    assert_eq!(ready.profile.as_deref(), Some("l40s-32k"));
    let restored = RunpodPool::restore(
        policy(),
        registry,
        transport.clone(),
        models(),
        first.snapshot(),
        Arc::new(clock.clone()),
    )
    .unwrap();
    let before = transport.calls();
    let status = restored.status(&qwen).unwrap();
    assert_eq!(transport.calls(), before);
    assert_eq!(status.phase, ModelPhase::Starting, "{status:?}");
    assert_eq!(status.sub_phase, None, "a stale state was reported");
    assert_eq!(status.deployment, Some(id("qwen-1")));
}

/// The pool's phase codes are exactly `llm-gateway.gateway.ModelPhase`'s, in order.
#[test]
fn every_pool_phase_is_declared_with_its_code() {
    let start = SPEC
        .find("  - name: llm-gateway.gateway.ModelPhase\n")
        .expect("spec/domains/gateway.yaml declares no ModelPhase");
    let rest = &SPEC[start + 1..];
    let declaration = &rest[..rest.find("\n  - name: ").unwrap_or(rest.len())];
    let declared: Vec<&str> = declaration
        .lines()
        .filter_map(|line| line.split_once("code: ").map(|(_, code)| code))
        .map(|code| code.trim_end_matches("}}"))
        .collect();
    let codes: Vec<&str> = ModelPhase::ALL.iter().map(|phase| phase.code()).collect();
    assert_eq!(declared, codes);
}
