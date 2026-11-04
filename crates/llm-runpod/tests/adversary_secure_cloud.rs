//! Adversary cases for story:secure-cloud-enforced: every pod the pool keeps is a Secure Cloud,
//! on-demand pod of an allowed image. A pod Runpod placed on a Community Cloud machine, an
//! interruptible pod, and a pod whose placement cannot be read are each terminated at once and
//! never handed out, and a pod still standing after a lost terminate is never ready and is
//! swept. In-process transport; nothing is opened.

use std::{collections::BTreeMap, sync::Arc};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    ALLOWED_IMAGE_PREFIX, AccountSpend, CloudType, ConfigError, CreateAnswer, EmulatedRunpod,
    ManualClock, PodListing, PodPlacement, PodRequest, PoolError, Probe, ProbeTarget, RunpodModel,
    RunpodPool, RunpodTransport, TerminateAnswer, Thinking, VllmSettings,
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

fn model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3.8-27B-FP8".to_owned(),
        image: id("vllm/vllm-openai:v0.27.1"),
        gpu_types: vec!["NVIDIA L40S".to_owned(), "NVIDIA H100 NVL".to_owned()],
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
        profile: "default".to_owned(),
        fallbacks: Vec::new(),
    }
}

fn pool<T: RunpodTransport>(transport: T, clock: &ManualClock) -> RunpodPool<T> {
    RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport,
        BTreeMap::from([(id(ALIAS), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool")
}

/// Asks until the pool hands out a lease, advancing the clock one second each time; returns
/// the endpoint it was handed.
fn served<T: RunpodTransport>(pool: &RunpodPool<T>, clock: &ManualClock) -> String {
    for _ in 0..20 {
        match pool.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => {
                return lease
                    .endpoint()
                    .expect("a lease has an endpoint")
                    .to_owned();
            }
            Err(PoolError::Starting | PoolError::NoCapacity) => clock.advance(1_000),
            Err(other) => panic!("unexpected refusal: {other:?}"),
        }
    }
    panic!("never served");
}

fn community() -> PodPlacement {
    PodPlacement {
        secure_cloud: Some(false),
        interruptible: None,
    }
}

fn interruptible() -> PodPlacement {
    PodPlacement {
        secure_cloud: Some(true),
        interruptible: Some(true),
    }
}

/// Each refused placement: the pod is terminated in the same `ensure` that created it, no
/// other GPU type is tried, the start is refused, and the pod never reaches a lease. The next
/// start places a Secure Cloud pod, and only that one is served.
#[test]
fn a_community_an_interruptible_and_an_unreadable_pod_are_terminated_at_once_and_never_served() {
    for (case, placement) in [
        ("community", Some(community())),
        ("interruptible", Some(interruptible())),
        ("unreadable", None),
        (
            "secure cloud unreported",
            Some(PodPlacement {
                secure_cloud: None,
                interruptible: Some(false),
            }),
        ),
    ] {
        let runpod = EmulatedRunpod::new();
        let clock = ManualClock::new(1_000);
        let pool = pool(runpod.clone(), &clock);
        runpod.place_next_create(placement);
        assert_eq!(
            pool.ensure(&id(ALIAS), &authorization()).err(),
            Some(PoolError::NoCapacity),
            "{case}: the start is refused"
        );
        assert_eq!(
            runpod.terminations(),
            ["pod1"],
            "{case}: terminated at once"
        );
        assert!(runpod.pods().is_empty(), "{case}: nothing left standing");
        assert_eq!(runpod.create_calls(), 1, "{case}: no second GPU type");
        assert_eq!(runpod.placement_reads(), 1, "{case}");
        let endpoint = served(&pool, &clock);
        assert!(
            !endpoint.contains("pod1"),
            "{case}: the refused pod was handed out: {endpoint}"
        );
        assert!(endpoint.contains("pod2"), "{case}: {endpoint}");
    }
}

/// Runpod omits `interruptible` for an on-demand pod: `secureCloud: true` with it absent is
/// accepted and served, and so is an explicit `false`.
#[test]
fn a_secure_cloud_pod_with_interruptible_absent_is_accepted() {
    for placement in [
        PodPlacement {
            secure_cloud: Some(true),
            interruptible: None,
        },
        PodPlacement {
            secure_cloud: Some(true),
            interruptible: Some(false),
        },
    ] {
        assert!(placement.is_secure_on_demand(), "{placement:?}");
        let runpod = EmulatedRunpod::new();
        let clock = ManualClock::new(1_000);
        let pool = pool(runpod.clone(), &clock);
        runpod.place_next_create(Some(placement));
        assert!(served(&pool, &clock).contains("pod1"), "{placement:?}");
        assert!(runpod.terminations().is_empty(), "{placement:?}");
    }
    for refused in [
        Some(community()),
        Some(interruptible()),
        Some(PodPlacement {
            secure_cloud: None,
            interruptible: None,
        }),
    ] {
        assert!(
            !refused.is_some_and(|placement| placement.is_secure_on_demand()),
            "{refused:?}"
        );
    }
}

/// Every create the pool sends asks for Secure Cloud and an on-demand pod.
#[test]
fn every_create_asks_for_secure_cloud_on_demand() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let pool = pool(runpod.clone(), &clock);
    runpod.no_capacity_gpu("NVIDIA L40S");
    drop(served(&pool, &clock));
    let requests = runpod.requests();
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert_eq!(request.cloud_type.as_str(), "SECURE");
        assert!(!request.interruptible);
        assert!(request.image.starts_with(ALLOWED_IMAGE_PREFIX));
    }
}

/// An image outside `vllm/vllm-openai:<tag>` is refused when the pool is built, before any
/// Runpod call.
#[test]
fn an_image_outside_the_allowed_prefix_is_refused_before_any_call() {
    for image in [
        "example/miner:v1",
        "vllm/vllm-openai",
        "vllm/vllm-openai:",
        "vllm/vllm-openai-fork:v1",
        "docker.io/vllm/vllm-openai:v1",
    ] {
        let mut declared = model();
        declared.image = id(image);
        assert_eq!(
            declared.validate(),
            Err(ConfigError::ImageNotAllowed),
            "{image}"
        );
        assert_eq!(ConfigError::ImageNotAllowed.code(), "image-not-allowed");
        assert!(!ConfigError::ImageNotAllowed.to_string().contains(image));
        let runpod = EmulatedRunpod::new();
        let refused = RunpodPool::new(
            policy(),
            Arc::new(LeaseRegistry::default()),
            runpod.clone(),
            BTreeMap::from([(id(ALIAS), declared)]),
            Arc::new(ManualClock::new(0)),
        )
        .err();
        assert_eq!(
            refused,
            Some(PoolError::InvalidModel(ConfigError::ImageNotAllowed)),
            "{image}"
        );
        assert_eq!(runpod.create_calls() + runpod.lists(), 0, "{image}");
    }
    assert!(ConfigError::ALL.contains(&ConfigError::ImageNotAllowed));
}

/// The emulator, with the answers to the first `lost` terminates lost (the pod stays).
struct LosingTerminates {
    inner: EmulatedRunpod,
    lost: Arc<std::sync::atomic::AtomicU32>,
    attempts: Arc<std::sync::atomic::AtomicU32>,
}

impl RunpodTransport for LosingTerminates {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.inner.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.inner.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        use std::sync::atomic::Ordering;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let left = self.lost.load(Ordering::SeqCst);
        if left > 0 {
            self.lost.store(left - 1, Ordering::SeqCst);
            return TerminateAnswer::Lost;
        }
        self.inner.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        self.inner.probe_ready(target)
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.inner.container_started_at(pod_id)
    }
    fn pod_placement(&mut self, pod_id: &str) -> Option<PodPlacement> {
        self.inner.pod_placement(pod_id)
    }
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        self.inner.account_spend(day_start_ms)
    }
}

/// A lost terminate is sent again, at most three times in all. A pod still standing after that
/// is never handed out, though it answers its probe ready, and the orphan sweep terminates it.
#[test]
fn a_refused_pod_whose_terminates_are_lost_is_never_served_and_is_swept() {
    use std::sync::atomic::{AtomicU32, Ordering};
    let runpod = EmulatedRunpod::new();
    let lost = Arc::new(AtomicU32::new(3));
    let attempts = Arc::new(AtomicU32::new(0));
    let clock = ManualClock::new(1_000);
    let pool = pool(
        LosingTerminates {
            inner: runpod.clone(),
            lost: Arc::clone(&lost),
            attempts: Arc::clone(&attempts),
        },
        &clock,
    );
    runpod.place_next_create(Some(community()));
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::NoCapacity)
    );
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        3,
        "three terminates, no more"
    );
    assert_eq!(runpod.pods().len(), 1, "the refused pod still stands");
    let endpoint = served(&pool, &clock);
    assert!(
        !endpoint.contains("pod1"),
        "the refused pod was handed out: {endpoint}"
    );
    let report = pool.reap().expect("reap");
    assert_eq!(report.orphans, vec![id("pod1")], "swept");
    assert_eq!(runpod.terminations(), ["pod1"]);
    assert_eq!(runpod.pods().len(), 1, "only the served pod stands");
}

/// A refused placement on the first profile ends the start: no fallback profile is tried.
#[test]
fn a_refused_placement_tries_no_fallback_profile() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let mut declared = model();
    declared.gpu_types = vec!["NVIDIA L40S".to_owned()];
    declared.fallbacks = vec![llm_runpod::RunpodProfile {
        name: "large".to_owned(),
        gpu_types: vec!["NVIDIA H100 NVL".to_owned()],
        vllm: declared.vllm.clone(),
    }];
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        BTreeMap::from([(id(ALIAS), declared)]),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    runpod.place_next_create(Some(interruptible()));
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::NoCapacity)
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(runpod.terminations(), ["pod1"]);
}
