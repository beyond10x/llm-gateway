//! Adversarial pass over story:model-profile-fallback: the one startup deadline of a start that
//! moves across profiles. Every test drives `EmulatedRunpod`; nothing reaches a provider API.

use std::{collections::BTreeMap, sync::Arc};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, CreateAnswer, EmulatedRunpod, HostingPolicy,
    Identifier, LeaseRegistry, ManualClock, PodListing, PodRequest, PoolError, Probe, ProbeTarget,
    RunpodModel, RunpodPool, RunpodProfile, RunpodTransport, TerminateAnswer, Thinking,
    VllmSettings,
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

fn vllm(max_model_len: u32) -> VllmSettings {
    VllmSettings {
        max_model_len,
        max_num_seqs: 8,
        gpu_memory_utilization: 0.9,
        thinking: Thinking::Off,
        reasoning_effort: None,
        sampling: None,
        extra_args: Vec::new(),
    }
}

/// `l40s-32k` (L40S, A40), then `h100-64k` (H100 NVL); a 600-second startup deadline.
fn model() -> RunpodModel {
    RunpodModel {
        hf_model: "Qwen/Qwen3.8-27B-FP8".to_owned(),
        image: id("vllm/vllm-openai:v0.27.1"),
        gpu_types: vec!["NVIDIA L40S".to_owned(), "NVIDIA A40".to_owned()],
        cloud_type: CloudType::Secure,
        container_disk_gb: 80,
        cache: None,
        data_center_ids: Vec::new(),
        vllm: vllm(32_768),
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 86_400_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "l40s-32k".to_owned(),
        fallbacks: vec![RunpodProfile {
            name: "h100-64k".to_owned(),
            gpu_types: vec!["NVIDIA H100 NVL".to_owned()],
            vllm: vllm(65_536),
        }],
    }
}

/// The emulator, with every create taking `create_ms` of the shared clock, as a `connectors`
/// write does (prepare, proof, invoke; up to `timeout_seconds`, 60 by default, per call).
struct Slow {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    create_ms: u64,
}

impl RunpodTransport for Slow {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.runpod.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.clock.advance(self.create_ms);
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
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        self.runpod.account_spend(day_start_ms)
    }
}

/// Acceptance 2: the next profile is tried "within the same startup deadline". Two creates of
/// the first profile, each refused for capacity after 400 s, spend 800 s of a 600 s deadline;
/// the start is past its deadline before the second profile is reached, so no create of it may
/// be sent. The provider tries every profile regardless (`RunpodProvider::create`, the loop
/// over `with_profile`, reads no clock) and sends an H100 create after the deadline the pool
/// counts from the submission (`deadline_from_ms = now`) has passed. Here the emulated pod is
/// ready at once and serves; a pod that still has to load is past its deadline from its
/// first probe on.
#[test]
fn adv_profiles_no_profile_is_tried_once_the_starts_deadline_has_passed() {
    let runpod = EmulatedRunpod::new();
    runpod.no_capacity_gpu("NVIDIA L40S");
    runpod.no_capacity_gpu("NVIDIA A40");
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        Slow {
            runpod: runpod.clone(),
            clock: clock.clone(),
            create_ms: 400_000,
        },
        BTreeMap::from([(id(ALIAS), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    let first = pool.ensure(&id(ALIAS), &authorization()).err();
    let requested: Vec<String> = runpod
        .requests()
        .iter()
        .map(|request| request.gpu_type.clone())
        .collect();
    let second = pool.ensure(&id(ALIAS), &authorization()).err();
    assert_eq!(
        (requested.as_slice(), runpod.terminations().len()),
        (
            ["NVIDIA L40S", "NVIDIA A40"].map(str::to_owned).as_slice(),
            0
        ),
        "the start, 800 s into a 600 s deadline, still created on the next profile; \
         first ensure {first:?}, second ensure {second:?}"
    );
    assert_ne!(second, Some(PoolError::StartupDeadline));
}
