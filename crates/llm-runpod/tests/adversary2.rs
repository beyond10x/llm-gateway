//! Second adversarial pass over the Runpod pool: the correction round, and ground the first pass
//! did not reach. Written against the hosting contract (`docs/hosting.md`) and the acceptance
//! statement, not against the implementation's own tests.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    CloudType, CreateAnswer, EmulatedRunpod, ManualClock, PodListing, PodRequest, PoolError, Probe,
    RunpodModel, RunpodPool, RunpodTransport, TerminateAnswer, Thinking, VllmSettings,
};

const ALIAS: &str = "qwen";

fn id(value: &str) -> Identifier {
    Identifier::new(value).expect("test identifier")
}

fn policy_for(controller: &str) -> HostingPolicy {
    HostingPolicy {
        controller: id(controller),
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
            max_model_len: 65_536,
            max_num_seqs: 8,
            gpu_memory_utilization: 0.9,
            thinking: Thinking::Off,
            reasoning_effort: None,
            sampling: None,
            extra_args: Vec::new(),
        },
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 1_800_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

fn models_for(alias: &str) -> BTreeMap<Identifier, RunpodModel> {
    BTreeMap::from([(id(alias), model())])
}

/// Steps `ensure` until it stops answering `starting`; returns the endpoint or the code.
fn settle<T: RunpodTransport>(
    pool: &RunpodPool<T>,
    clock: &ManualClock,
) -> Result<Option<String>, &'static str> {
    for _ in 0..20 {
        match pool.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => return Ok(lease.endpoint().map(str::to_owned)),
            Err(PoolError::Starting) => clock.advance(1_000),
            Err(other) => return Err(other.code()),
        }
    }
    Err("still-starting")
}

// --- ownership: the contract's takeover sequence -----------------------------------------------

/// The hosting contract names "lease expiry, reacquisition by another controller, Observe, Stop"
/// as an ordinary sequence (`docs/hosting.md`, the precedence rule) and says an expired claim
/// may be taken over. The Runpod adapter writes the owner into the pod's environment at create
/// time and has no transport operation that can ever rewrite it. So the new owner observes its
/// inherited pod still labelled for the old owner at an older epoch, the contract parks the
/// record in `ownership-lost`, which `submit_owed_stops` never submits, and the new owner's
/// sweep skips the pod because its tag names the old owner. The pod keeps billing and the alias
/// answers `stopping`.
#[test]
fn a_pod_inherited_through_a_lease_takeover_is_served_or_stopped_by_its_new_owner() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let leases = Arc::new(LeaseRegistry::default());
    let first = RunpodPool::new(
        policy_for("controller-a"),
        Arc::clone(&leases),
        runpod.clone(),
        models_for(ALIAS),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    settle(&first, &clock).expect("controller-a serves");
    let snapshot = first.snapshot();
    drop(first);
    // controller-a is gone; its claim runs out and controller-b takes the records over.
    clock.advance(3_600_001);
    let second = RunpodPool::restore(
        policy_for("controller-b"),
        leases,
        runpod.clone(),
        models_for(ALIAS),
        snapshot,
        Arc::new(clock.clone()),
    )
    .expect("restore under the new owner");
    let mut answers = Vec::new();
    for _ in 0..5 {
        answers.push(
            second
                .ensure(&id(ALIAS), &authorization())
                .map(|lease| lease.endpoint().map(str::to_owned))
                .map_err(PoolError::code),
        );
        second.reap().expect("reap");
        clock.advance(1_000);
    }
    let pod_alive = runpod.pods().iter().any(|pod| pod.id == "pod-1");
    let served = answers.iter().any(Result::is_ok);
    let record = second
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment == id("qwen-1"))
        .map(|record| (record.phase, record.stop_reason));
    assert!(
        served || !pod_alive,
        "the inherited pod-1 is neither served nor stopped by the new owner: answers {answers:?}, \
         record {record:?}, terminations {:?}",
        runpod.terminations()
    );
}

// --- readiness recovery ------------------------------------------------------------------------

/// Answers one chosen pod's readiness probe with a definite "not ready", as a vLLM whose engine
/// died behind a live HTTP server does. The emulator can only make a pod unready before it has
/// served once.
#[derive(Clone)]
struct Wedging {
    inner: EmulatedRunpod,
    wedged: Arc<Mutex<Option<String>>>,
}

impl RunpodTransport for Wedging {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.inner.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.inner.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.inner.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, pod_id: &str) -> Probe {
        let wedged = self
            .wedged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let answer = self.inner.probe_ready(pod_id);
        if wedged.as_deref() == Some(pod_id) {
            Probe::NotReady
        } else {
            answer
        }
    }
    fn container_uptime(&mut self, pod_id: &str) -> Option<u64> {
        self.inner.container_uptime(pod_id)
    }
}

/// Acceptance: "readiness/crash recovery". A pod that served once and then answers "not ready"
/// for longer than its whole declared startup deadline is a billed resource serving nobody —
/// the doc's own reason for the deadline. `ready_seen` switches the deadline off for good once
/// a pod has served, so the pool answers `starting` until the idle reaper happens to fire, a
/// whole idle timeout later (30 minutes here), and never replaces it on its own.
#[test]
fn a_pod_that_stops_serving_after_it_served_is_retired_within_its_startup_deadline() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let wedged = Arc::new(Mutex::new(None));
    let transport = Wedging {
        inner: runpod.clone(),
        wedged: Arc::clone(&wedged),
    };
    let pool = RunpodPool::new(
        policy_for("controller-a"),
        Arc::new(LeaseRegistry::default()),
        transport,
        models_for(ALIAS),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    settle(&pool, &clock).expect("serves");
    *wedged.lock().unwrap_or_else(PoisonError::into_inner) = Some("pod-1".to_owned());
    let mut answers = Vec::new();
    // 11 x 60 s: past the 600 s startup deadline, well inside the 1 800 s idle timeout.
    for _ in 0..11 {
        clock.advance(60_000);
        answers.push(
            pool.ensure(&id(ALIAS), &authorization())
                .map(|_| ())
                .map_err(PoolError::code),
        );
        pool.reap().expect("reap");
    }
    assert!(
        runpod.terminations().contains(&"pod-1".to_owned()),
        "pod-1 has not served for 660 s against a 600 s deadline and is still billed; \
         answers {answers:?}"
    );
}

// --- request identity: the correction's new format ----------------------------------------------

/// F1 made the request id `request-<controller>-<alias>-<generation>-<instant>-<nonce>`. Both
/// the controller id and the alias are hosting identifiers, valid up to 256 bytes each, and the
/// pool accepts both at construction. Their composition is itself an identifier and overflows
/// 256 bytes long before either part does, so every create is refused as `hosting` with nothing
/// said at construction time.
#[test]
fn a_pool_that_accepts_its_controller_and_alias_can_start_a_pod() {
    let controller = "c".repeat(120);
    let alias = "q".repeat(120);
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy_for(&controller),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        models_for(&alias),
        Arc::new(clock.clone()),
    );
    let Ok(pool) = pool else {
        return;
    };
    let answer = pool.ensure(&id(&alias), &authorization()).err();
    assert_eq!(
        answer,
        Some(PoolError::Starting),
        "a pool constructed over a 120-byte controller id and a 120-byte alias refuses every \
         create (create calls: {})",
        runpod.create_calls()
    );
}
