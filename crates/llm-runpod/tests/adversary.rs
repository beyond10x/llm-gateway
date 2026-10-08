//! Adversarial cases for the Runpod pool, written against the hosting contract rather than
//! against the implementation's own tests.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry, Phase};
use llm_runpod::{
    CloudType, CreateAnswer, EmulatedRunpod, LEGACY_POD_NAME_PREFIX, ManualClock, NetworkVolume,
    POD_NAME_PREFIX, PodListing, PodRequest, PodStatus, PoolError, Probe, ProbeTarget, RunpodModel,
    RunpodPool, RunpodTransport, TAG_EPOCH, TAG_OWNER, TAG_REQUEST, TerminateAnswer, Thinking,
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
            extra_args: vec!["--kv-cache-dtype".to_owned(), "fp8".to_owned()],
        },
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: 600_000,
        idle_timeout_ms: 1_800_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

fn models() -> BTreeMap<Identifier, RunpodModel> {
    BTreeMap::from([(id(ALIAS), model())])
}

fn tags(owner: &str, epoch: u64, request: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (TAG_OWNER.to_owned(), owner.to_owned()),
        (TAG_EPOCH.to_owned(), epoch.to_string()),
        (TAG_REQUEST.to_owned(), request.to_owned()),
    ])
}

fn pool_over<T: RunpodTransport>(transport: T, clock: &ManualClock) -> RunpodPool<T> {
    RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport,
        models(),
        Arc::new(clock.clone()),
    )
    .expect("pool")
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

// --- request identity ------------------------------------------------------------------------

/// Two controllers running this pool over one Runpod account — the setting the owner tag exists
/// for — derive the same request id (`request-<alias>-<generation>`) for the same alias. A lost
/// create is resolved by request id, so controller A adopts controller B's pod as its own
/// answer, finds a foreign owner on it, and parks its record in `stop-required/ownership-lost`,
/// which nothing ever submits. A's own pod is protected from the sweep by the same request id.
#[test]
fn a_lost_create_is_not_resolved_against_another_controllers_pod_with_the_same_alias() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let theirs = runpod.insert_pod(
        &format!("{POD_NAME_PREFIX}{ALIAS}"),
        tags("controller-b", 1, "request-qwen-1"),
    );
    let pool = pool_over(runpod.clone(), &clock);
    runpod.lose_next_create(true);
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    let ours = runpod
        .pods()
        .into_iter()
        .map(|pod| pod.id)
        .find(|pod| pod != &theirs)
        .expect("the lost create allocated a pod");
    assert_eq!(
        settle(&pool, &clock),
        Ok(Some(format!("https://{ours}-8000.proxy.runpod.net/v1/"))),
        "the lost create must resolve to the pod it created, not to another controller's pod \
         that happens to carry the same deterministic request id"
    );
}

/// A pool opened with `new` over an account that still holds one of this controller's pods —
/// a restart whose snapshot was not persisted; nothing in this crate persists it — reuses
/// generation 1 and therefore the leftover pod's request id. The sweep treats any pod whose
/// request id matches a live record as recorded, so the leftover is billed for as long as the
/// new record lives, although that record already owns a different, exact key.
#[test]
fn a_leftover_pod_of_ours_is_swept_even_when_a_fresh_pool_reuses_its_request_id() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let leftover = runpod.insert_pod(
        &format!("{POD_NAME_PREFIX}{ALIAS}"),
        tags("controller-a", 1, "request-qwen-1"),
    );
    let pool = pool_over(runpod.clone(), &clock);
    let endpoint = settle(&pool, &clock).expect("serves");
    assert_ne!(
        endpoint,
        Some(format!("https://{leftover}-8000.proxy.runpod.net/v1/")),
        "precondition: the new record owns a different pod"
    );
    let report = pool.reap().expect("reap");
    assert_eq!(
        report.orphans,
        vec![id(&leftover)],
        "a pod tagged as ours that no record holds under its exact key is an orphan"
    );
}

// --- provider state --------------------------------------------------------------------------

/// Reports one chosen pod as `EXITED`, as Runpod does for a stopped (not terminated) pod.
#[derive(Clone)]
struct Exiting {
    inner: EmulatedRunpod,
    exited: Arc<Mutex<Option<String>>>,
}

impl RunpodTransport for Exiting {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        let exited = self
            .exited
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut listing = self.inner.list_pods()?;
        for pod in &mut listing.pods {
            if exited.as_deref() == Some(pod.id.as_str()) {
                pod.status = PodStatus::Exited;
            }
        }
        Ok(listing)
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.inner.create_pod(request)
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.inner.terminate_pod(pod_id)
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        self.inner.probe_ready(target)
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.inner.container_started_at(pod_id)
    }
}

/// `PodStatus::Exited` is mapped to `ProviderState::Terminated`, so the controller discharges
/// the record with `provider-terminated` evidence while the pod still exists in the account,
/// and the sweep skips it forever because it reads as terminated.
#[test]
fn an_exited_pod_is_terminated_rather_than_recorded_as_stopped() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let exited = Arc::new(Mutex::new(None));
    let transport = Exiting {
        inner: runpod.clone(),
        exited: Arc::clone(&exited),
    };
    let pool = pool_over(transport, &clock);
    settle(&pool, &clock).expect("serves");
    *exited.lock().unwrap_or_else(PoisonError::into_inner) = Some("pod1".to_owned());
    clock.advance(1_000);
    let _ = pool.ensure(&id(ALIAS), &authorization());
    pool.reap().expect("reap");
    let first = pool
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment == id("qwen-1"))
        .expect("record");
    let still_listed = runpod.pods().iter().any(|pod| pod.id == "pod1");
    assert!(
        !(first.phase == Phase::Stopped && still_listed),
        "qwen-1 is {:?} with evidence {:?}, yet pod1 still exists and was never terminated \
         (terminations: {:?})",
        first.phase,
        first.stop_evidence,
        runpod.terminations()
    );
}

// --- mutants the suite does not kill ---------------------------------------------------------

/// Exactly `crash_restart_limit` restarts make a pod crash-looping. The existing case scripts
/// three restarts for a limit of two, so `>=` weakened to `>` still passes it.
#[test]
fn the_crash_restart_limit_is_reached_at_exactly_its_count() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    runpod.ready_after(u32::MAX);
    runpod.uptimes(vec![30, 5, 40, 2]);
    let pool = pool_over(runpod.clone(), &clock);
    assert_eq!(settle(&pool, &clock), Err("crash-loop"));
    assert_eq!(runpod.terminations(), vec!["pod1".to_owned()]);
}

/// Every setting validation claims to refuse, one at a time. The existing case leaves these
/// clauses individually unexercised.
#[test]
fn every_refused_setting_is_refused_on_its_own() {
    type Breakage = (&'static str, fn(&mut RunpodModel));
    let cases: &[Breakage] = &[
        ("gpu-utilization", |m| m.vllm.gpu_memory_utilization = 0.0),
        ("gpu-utilization", |m| m.vllm.gpu_memory_utilization = -0.5),
        ("zero-setting", |m| m.vllm.max_num_seqs = 0),
        ("zero-setting", |m| m.container_disk_gb = 0),
        ("secret-reference", |m| m.api_key_secret = String::new()),
        ("unsafe-text", |m| {
            m.vllm.reasoning_effort = Some(String::new());
        }),
        ("unsafe-text", |m| m.gpu_types = vec!["L40S\n".to_owned()]),
        ("unsafe-text", |m| {
            m.data_center_ids = vec!["US CA".to_owned()];
        }),
        ("unsafe-text", |m| m.vllm.extra_args = vec![String::new()]),
        ("unsafe-text", |m| m.vllm.sampling = Some("{\n}".to_owned())),
        ("unsafe-text", |m| {
            m.cache = Some(NetworkVolume {
                volume_id: "vol 1".to_owned(),
                mount_path: "/workspace".to_owned(),
            });
        }),
    ];
    for (code, break_it) in cases {
        let mut declared = model();
        break_it(&mut declared);
        assert_eq!(
            declared.validate().map_err(llm_runpod::ConfigError::code),
            Err(*code),
            "{code}"
        );
    }
    let mut boundary = model();
    boundary.vllm.gpu_memory_utilization = 1.0;
    assert_eq!(boundary.validate(), Ok(()), "1.0 is inside (0, 1]");
}

/// llmgw pods carry no request tag. If one were listed, no listing could ever rule a lost
/// create out, and every lost create would block its model for good. The inventory's namespace
/// filter is what prevents that; nothing else in the suite depends on it.
#[test]
fn an_llmgw_pod_does_not_keep_a_lost_create_unresolved() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    runpod.insert_pod(&format!("{LEGACY_POD_NAME_PREFIX}{ALIAS}"), BTreeMap::new());
    let pool = pool_over(runpod.clone(), &clock);
    runpod.lose_next_create(false);
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert!(
        settle(&pool, &clock).is_ok(),
        "the next request starts afresh"
    );
    assert_eq!(runpod.create_calls(), 2);
}
