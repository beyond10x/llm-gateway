//! Emulated Runpod lifecycle: single-flight startup, readiness and crash recovery,
//! ownership-safe adoption, and reachable idle and orphan cleanup, all driven by declared vLLM
//! settings.
//!
//! Ported from the mock lifecycle tests in llmgw `src/lib.rs` (`concurrent_first_requests_create_
//! exactly_one_pod`, `an_idle_pod_is_reaped_to_zero_and_cold_starts_again`, `an_existing_pod_is_
//! adopted_by_name_instead_of_created`, `a_crash_looping_pod_is_terminated_and_refused`,
//! `an_endpoint_that_stops_answering_is_dropped_and_replaced`, `the_orphan_sweep_terminates_
//! unregistered_gateway_pods_only`). Those ran an HTTP mock on a local socket; these drive an
//! in-process transport and open nothing.

use std::{
    collections::BTreeMap,
    sync::{Arc, Barrier},
    thread,
};

use llm_provision::{
    ComputeAuthorization, Controller, HostingCommand, HostingError, HostingPolicy, Identifier,
    LeaseRegistry, Phase,
};
use llm_runpod::{
    CloudType, EmulatedRunpod, LEGACY_POD_NAME_PREFIX, ManualClock, NetworkVolume, POD_NAME_PREFIX,
    PodStatus, PoolError, RunpodModel, RunpodPool, RunpodProvider, TAG_EPOCH, TAG_OWNER,
    TAG_REQUEST, Thinking, VllmSettings,
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

struct Stack {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    leases: Arc<LeaseRegistry>,
    pool: Arc<RunpodPool<EmulatedRunpod>>,
}

fn stack_with(models: BTreeMap<Identifier, RunpodModel>) -> Stack {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let leases = Arc::new(LeaseRegistry::default());
    let pool = RunpodPool::new(
        policy(),
        Arc::clone(&leases),
        runpod.clone(),
        models,
        Arc::new(clock.clone()),
    )
    .expect("pool");
    Stack {
        runpod,
        clock,
        leases,
        pool: Arc::new(pool),
    }
}

fn stack() -> Stack {
    stack_with(models())
}

/// Calls `ensure` until the pool hands out a ready lease, advancing the clock a little each time.
fn ready(stack: &Stack) -> llm_runpod::StreamLease {
    for _ in 0..20 {
        match stack.pool.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => return lease,
            Err(PoolError::Starting) => stack.clock.advance(1_000),
            Err(other) => panic!("unexpected refusal while starting: {}", other.code()),
        }
    }
    panic!("the pod never became ready");
}

fn restart(
    stack: &Stack,
    models: BTreeMap<Identifier, RunpodModel>,
) -> Arc<RunpodPool<EmulatedRunpod>> {
    let snapshot = stack.pool.snapshot();
    Arc::new(
        RunpodPool::restore(
            policy(),
            Arc::clone(&stack.leases),
            stack.runpod.clone(),
            models,
            snapshot,
            Arc::new(stack.clock.clone()),
        )
        .expect("restore"),
    )
}

// --- single-flight startup -------------------------------------------------------------------

#[test]
fn concurrent_first_requests_create_exactly_one_pod() {
    let stack = stack();
    let barrier = Arc::new(Barrier::new(10));
    let mut callers = Vec::new();
    for _ in 0..10 {
        let pool = Arc::clone(&stack.pool);
        let barrier = Arc::clone(&barrier);
        callers.push(thread::spawn(move || {
            barrier.wait();
            pool.ensure(&id(ALIAS), &authorization()).map(drop)
        }));
    }
    for caller in callers {
        match caller.join().expect("caller thread") {
            Ok(()) | Err(PoolError::Starting) => {}
            Err(other) => panic!("unexpected refusal: {}", other.code()),
        }
    }
    assert_eq!(stack.runpod.create_calls(), 1, "ten callers, one create");
    drop(ready(&stack));
    assert_eq!(
        stack.runpod.create_calls(),
        1,
        "becoming ready creates nothing"
    );
}

#[test]
fn a_request_while_the_pod_starts_waits_on_it_instead_of_creating_another() {
    let stack = stack();
    stack.runpod.ready_after(3);
    for _ in 0..3 {
        assert_eq!(
            stack.pool.ensure(&id(ALIAS), &authorization()).err(),
            Some(PoolError::Starting)
        );
    }
    let lease = ready(&stack);
    assert_eq!(stack.runpod.create_calls(), 1);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-1-8000.proxy.runpod.net"),
        "the endpoint is derived from the pod the provider reported"
    );
    assert_eq!(lease.served_model(), Some(&id(ALIAS)));
}

#[test]
fn an_unknown_model_is_refused_without_contacting_runpod() {
    let stack = stack();
    assert_eq!(
        stack.pool.ensure(&id("absent"), &authorization()).err(),
        Some(PoolError::UnknownModel)
    );
    assert_eq!(stack.runpod.create_calls(), 0);
    assert_eq!(stack.runpod.lists(), 0);
}

// --- declared vLLM settings and GPU choice ---------------------------------------------------

#[test]
fn the_create_request_carries_the_declared_vllm_settings() {
    let mut declared = model();
    declared.cache = Some(NetworkVolume {
        volume_id: "volume-1".to_owned(),
        mount_path: "/workspace".to_owned(),
    });
    declared.data_center_ids = vec!["US-CA-2".to_owned()];
    let stack = stack_with(BTreeMap::from([(id(ALIAS), declared)]));
    drop(ready(&stack));
    let requests = stack.runpod.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.name, format!("{POD_NAME_PREFIX}{ALIAS}"));
    assert_eq!(request.image, "vllm/vllm-openai:v0.27.1");
    assert_eq!(request.gpu_type, "NVIDIA L40S");
    assert_eq!(request.gpu_count, 1);
    assert_eq!(request.cloud_type, CloudType::Secure);
    assert_eq!(request.container_disk_gb, 80);
    assert_eq!(request.ports, vec!["8000/http".to_owned()]);
    assert_eq!(request.network_volume_id.as_deref(), Some("volume-1"));
    assert_eq!(request.volume_mount_path.as_deref(), Some("/workspace"));
    assert_eq!(request.data_center_ids, vec!["US-CA-2".to_owned()]);
    let joined = request.docker_entrypoint.join(" ");
    assert!(joined.starts_with("vllm serve Qwen/Qwen3.8-27B-FP8 --host 0.0.0.0 --port 8000"));
    assert!(joined.contains(&format!("--served-model-name {ALIAS}")));
    assert!(joined.contains("--max-model-len 65536"));
    assert!(joined.contains("--max-num-seqs 8"));
    assert!(joined.contains("--gpu-memory-utilization 0.9"));
    assert!(joined.contains(r#"--default-chat-template-kwargs {"enable_thinking":false}"#));
    assert!(joined.contains("--override-generation-config"));
    assert!(
        joined.ends_with("--kv-cache-dtype fp8"),
        "extra arguments come last"
    );
    assert!(
        !joined.contains("--api-key"),
        "no credential value travels in the entrypoint"
    );
    assert_eq!(
        request.env.get("VLLM_API_KEY").map(String::as_str),
        Some("{{ RUNPOD_SECRET_qwen_vllm_key }}"),
        "the vLLM key is a reference to a Runpod secret, never a value"
    );
    assert_eq!(
        request.env.get("HF_HOME").map(String::as_str),
        Some("/workspace/huggingface"),
        "weights are cached on the mounted volume"
    );
    assert_eq!(
        request.env.get(TAG_OWNER).map(String::as_str),
        Some("controller-a")
    );
    assert_eq!(request.env.get(TAG_EPOCH).map(String::as_str), Some("1"));
    let recorded = stack.pool.view().deployments.remove(0).request_id;
    assert_eq!(
        request.env.get(TAG_REQUEST).map(String::as_str),
        Some(recorded.as_str()),
        "the pod carries exactly the request id the record holds"
    );
    assert!(
        recorded
            .as_str()
            .starts_with("request-controller-a-qwen-1-"),
        "the request id names the controller, the deployment and a nonce: {recorded}"
    );
}

#[test]
fn a_model_without_a_volume_carries_no_placement_or_cache_override() {
    let stack = stack();
    drop(ready(&stack));
    let request = &stack.runpod.requests()[0];
    assert_eq!(request.network_volume_id, None);
    assert_eq!(request.volume_mount_path, None);
    assert_eq!(request.data_center_ids, [] as [String; 0]);
    assert!(!request.env.contains_key("HF_HOME"));
}

#[test]
fn reasoning_effort_and_explicit_sampling_reach_the_entrypoint() {
    let mut declared = model();
    declared.vllm.thinking = Thinking::On;
    declared.vllm.reasoning_effort = Some("medium".to_owned());
    declared.vllm.sampling = Some(r#"{"temperature":0.2}"#.to_owned());
    let stack = stack_with(BTreeMap::from([(id(ALIAS), declared)]));
    drop(ready(&stack));
    let joined = stack.runpod.requests()[0].docker_entrypoint.join(" ");
    assert!(joined.contains(r#"--default-chat-template-kwargs {"reasoning_effort":"medium"}"#));
    assert!(joined.contains(r#"--override-generation-config {"temperature":0.2}"#));
    assert!(!joined.contains("enable_thinking"));
}

#[test]
fn a_placement_refusal_falls_through_to_the_next_declared_gpu_in_order() {
    let stack = stack();
    stack.runpod.refuse_gpu("NVIDIA L40S");
    drop(ready(&stack));
    let gpus: Vec<_> = stack
        .runpod
        .requests()
        .into_iter()
        .map(|request| request.gpu_type)
        .collect();
    assert_eq!(
        gpus,
        vec!["NVIDIA L40S".to_owned(), "NVIDIA H100 NVL".to_owned()]
    );
    assert_eq!(stack.runpod.pods().len(), 1);
}

#[test]
fn every_declared_gpu_refusing_withdraws_the_deployment_and_leaves_no_pod() {
    let stack = stack();
    stack.runpod.refuse_gpu("NVIDIA L40S");
    stack.runpod.refuse_gpu("NVIDIA H100 NVL");
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::NoCapacity)
    );
    assert_eq!(stack.runpod.pods(), [] as [llm_runpod::Pod; 0]);
    let phases: Vec<_> = stack
        .pool
        .view()
        .deployments
        .into_iter()
        .map(|record| record.phase)
        .collect();
    assert_eq!(phases, vec![Phase::Cancelled]);
}

#[test]
fn a_lost_create_answer_is_never_retried_on_another_gpu_and_is_adopted_by_request_id() {
    let stack = stack();
    stack.runpod.lose_next_create(true);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(
        stack.runpod.create_calls(),
        1,
        "a lost answer may have allocated; trying the next GPU could pay twice"
    );
    let lease = ready(&stack);
    assert_eq!(stack.runpod.create_calls(), 1, "and it is never retried");
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-1-8000.proxy.runpod.net")
    );
}

#[test]
fn a_lost_create_that_allocated_nothing_is_discharged_and_the_next_request_starts_afresh() {
    let stack = stack();
    stack.runpod.lose_next_create(false);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    drop(ready(&stack));
    assert_eq!(stack.runpod.create_calls(), 2);
    assert_eq!(stack.runpod.pods().len(), 1);
}

// --- readiness and crash recovery ------------------------------------------------------------

#[test]
fn a_crash_looping_pod_is_terminated_and_the_next_request_starts_a_fresh_one() {
    let stack = stack();
    stack.runpod.ready_after(u32::MAX);
    stack.runpod.uptimes(vec![30, 5, 40, 2, 50, 1]);
    let mut refusal = None;
    for _ in 0..10 {
        match stack.pool.ensure(&id(ALIAS), &authorization()) {
            Err(PoolError::Starting) => stack.clock.advance(1_000),
            other => {
                refusal = Some(other.map(drop));
                break;
            }
        }
    }
    assert_eq!(refusal, Some(Err(PoolError::CrashLoop)));
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
    assert_eq!(stack.runpod.pods(), [] as [llm_runpod::Pod; 0]);
    stack.runpod.ready_after(0);
    stack.runpod.uptimes(Vec::new());
    let lease = ready(&stack);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-2-8000.proxy.runpod.net")
    );
    assert_eq!(stack.runpod.create_calls(), 2);
}

#[test]
fn a_pod_that_refuses_its_credential_is_terminated() {
    let stack = stack();
    stack.runpod.ready_after(1);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.runpod.refuse_credential("pod-1");
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::CredentialRefused)
    );
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
}

#[test]
fn a_pod_that_misses_its_startup_deadline_is_terminated() {
    let stack = stack();
    stack.runpod.ready_after(u32::MAX);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.clock.advance(600_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting),
        "the deadline is inclusive"
    );
    stack.clock.advance(1);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::StartupDeadline)
    );
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
}

#[test]
fn a_pod_that_vanishes_out_of_band_is_replaced_on_the_next_request() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.vanish("pod-1");
    let lease = ready(&stack);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-2-8000.proxy.runpod.net")
    );
    assert_eq!(stack.runpod.create_calls(), 2);
    assert!(
        stack.runpod.terminations().is_empty(),
        "nothing to terminate"
    );
}

#[test]
fn a_partial_listing_neither_discharges_nor_replaces_a_running_pod() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.partial_listing(true);
    stack.runpod.vanish("pod-1");
    stack.clock.advance(1_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting),
        "absence from a partial listing says nothing"
    );
    assert_eq!(stack.runpod.create_calls(), 1);
}

#[test]
fn an_unreported_readiness_or_served_model_stays_unknown() {
    let stack = stack();
    stack.runpod.probe_unreachable(true);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    let record = stack.pool.view().deployments.remove(0);
    let observed = record.observed.expect("observed");
    assert_eq!(observed.ready, None, "an unanswered probe is not `false`");
    assert_eq!(
        observed.served_model, None,
        "the requested model is not a served model"
    );
}

// --- ownership-safe adoption -----------------------------------------------------------------

#[test]
fn a_restarted_pool_adopts_its_own_pod_by_exact_identity_without_creating() {
    let stack = stack();
    drop(ready(&stack));
    let restarted = restart(&stack, models());
    let lease = restarted
        .ensure(&id(ALIAS), &authorization())
        .expect("adopted pod serves");
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-1-8000.proxy.runpod.net")
    );
    assert_eq!(stack.runpod.create_calls(), 1, "adoption creates nothing");
    assert_eq!(stack.runpod.terminations(), [] as [String; 0]);
}

#[test]
fn a_restarted_pool_does_not_adopt_a_pod_that_reused_its_name() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.vanish("pod-1");
    let foreign = stack.runpod.insert_pod(
        &format!("{POD_NAME_PREFIX}{ALIAS}"),
        BTreeMap::from([
            (TAG_OWNER.to_owned(), "controller-b".to_owned()),
            (TAG_EPOCH.to_owned(), "7".to_owned()),
            (TAG_REQUEST.to_owned(), "their-request".to_owned()),
        ]),
    );
    let restarted = restart(&stack, models());
    let mut attempts = 0;
    let lease = loop {
        attempts += 1;
        assert!(attempts <= 20, "the restarted pool never became ready");
        match restarted.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => break lease,
            Err(PoolError::Starting) => stack.clock.advance(1_000),
            Err(other) => panic!("{}", other.code()),
        }
    };
    assert_ne!(
        lease.endpoint(),
        Some(format!("https://{foreign}-8000.proxy.runpod.net").as_str()),
        "the same name on a different pod is a different resource"
    );
    assert_eq!(stack.runpod.create_calls(), 2);
    assert!(
        !stack.runpod.terminations().contains(&foreign),
        "another owner's pod is never terminated"
    );
}

#[test]
fn a_pod_retagged_by_a_newer_owner_is_handed_over_and_never_terminated() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.retag("pod-1", "controller-b", 99);
    stack.clock.advance(1_000);
    let _ = stack.pool.ensure(&id(ALIAS), &authorization());
    let report = stack.pool.reap().expect("reap");
    assert_eq!(report.orphans, [] as [llm_provision::Identifier; 0]);
    assert!(
        !stack.runpod.terminations().contains(&"pod-1".to_owned()),
        "the newer owner's pod is not ours to stop"
    );
    assert!(
        stack.pool.view().totals.transferred.contains(&id("qwen-1")),
        "the obligation moved with the ownership and says so"
    );
}

// --- reachable idle and orphan cleanup -------------------------------------------------------

#[test]
fn an_idle_pod_is_reaped_and_the_next_request_cold_starts_again() {
    let stack = stack();
    drop(ready(&stack));
    stack.clock.advance(1_799_999);
    assert_eq!(
        stack.pool.reap().expect("reap").idle,
        [] as [llm_provision::Identifier; 0]
    );
    assert_eq!(stack.runpod.terminations(), [] as [String; 0]);
    stack.clock.advance(1);
    let report = stack.pool.reap().expect("reap");
    assert_eq!(report.idle, vec![id("qwen-1")]);
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
    let lease = ready(&stack);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-2-8000.proxy.runpod.net")
    );
}

#[test]
fn an_open_stream_lease_keeps_the_pod_from_being_reaped() {
    let stack = stack();
    let lease = ready(&stack);
    stack.clock.advance(3_000_000);
    assert_eq!(
        stack.pool.reap().expect("reap").idle,
        [] as [llm_provision::Identifier; 0]
    );
    assert_eq!(stack.runpod.terminations(), [] as [String; 0]);
    drop(lease);
    stack.clock.advance(1_799_999);
    assert!(
        stack.pool.reap().expect("reap").idle.is_empty(),
        "the idle window starts when the stream ends"
    );
    stack.clock.advance(1);
    assert_eq!(stack.pool.reap().expect("reap").idle, vec![id("qwen-1")]);
}

#[test]
fn the_idle_limit_is_never_below_the_measured_cold_start() {
    let mut declared = model();
    declared.idle_timeout_ms = 60_000;
    let stack = stack_with(BTreeMap::from([(id(ALIAS), declared)]));
    stack.runpod.ready_after(240);
    drop(ready_slowly(&stack, 1_000));
    // The start took about 240 s, so one idle minute is not worth paying that again.
    stack.clock.advance(60_000);
    assert_eq!(
        stack.pool.reap().expect("reap").idle,
        [] as [llm_provision::Identifier; 0]
    );
    stack.clock.advance(181_000);
    assert_eq!(stack.pool.reap().expect("reap").idle, vec![id("qwen-1")]);
}

fn ready_slowly(stack: &Stack, step_ms: u64) -> llm_runpod::StreamLease {
    for _ in 0..1_000 {
        match stack.pool.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => return lease,
            Err(PoolError::Starting) => stack.clock.advance(step_ms),
            Err(other) => panic!("{}", other.code()),
        }
    }
    panic!("never ready");
}

#[test]
fn the_orphan_sweep_terminates_only_this_controllers_unrecorded_pods() {
    let stack = stack();
    drop(ready(&stack));
    let ours = |request: &str| {
        BTreeMap::from([
            (TAG_OWNER.to_owned(), "controller-a".to_owned()),
            (TAG_EPOCH.to_owned(), "1".to_owned()),
            (TAG_REQUEST.to_owned(), request.to_owned()),
        ])
    };
    let orphan = stack.runpod.insert_pod(
        &format!("{POD_NAME_PREFIX}removed"),
        ours("request-removed-1"),
    );
    let foreign = stack.runpod.insert_pod(
        &format!("{POD_NAME_PREFIX}theirs"),
        BTreeMap::from([(TAG_OWNER.to_owned(), "controller-b".to_owned())]),
    );
    let untagged = stack
        .runpod
        .insert_pod(&format!("{POD_NAME_PREFIX}untagged"), BTreeMap::new());
    let unrelated = stack.runpod.insert_pod("someone-elses-pod", ours("x"));
    let report = stack.pool.reap().expect("reap");
    assert_eq!(report.orphans, vec![id(&orphan)]);
    assert_eq!(stack.runpod.terminations(), vec![orphan]);
    for kept in [foreign, untagged, unrelated, "pod-1".to_owned()] {
        assert!(
            stack.runpod.pods().iter().any(|pod| pod.id == kept),
            "{kept} must survive the sweep"
        );
    }
}

#[test]
fn the_orphan_sweep_never_selects_a_legacy_llmgw_pod() {
    assert_ne!(POD_NAME_PREFIX, LEGACY_POD_NAME_PREFIX);
    assert!(!POD_NAME_PREFIX.starts_with(LEGACY_POD_NAME_PREFIX));
    assert!(!LEGACY_POD_NAME_PREFIX.starts_with(POD_NAME_PREFIX));
    let stack = stack();
    // Even tagged exactly as this controller would tag its own pod.
    let legacy = stack.runpod.insert_pod(
        &format!("{LEGACY_POD_NAME_PREFIX}{ALIAS}"),
        BTreeMap::from([
            (TAG_OWNER.to_owned(), "controller-a".to_owned()),
            (TAG_EPOCH.to_owned(), "1".to_owned()),
            (TAG_REQUEST.to_owned(), "request-qwen-1".to_owned()),
        ]),
    );
    let report = stack.pool.reap().expect("reap");
    assert_eq!(report.orphans, [] as [llm_provision::Identifier; 0]);
    drop(ready(&stack));
    assert_eq!(
        stack.runpod.create_calls(),
        1,
        "the legacy pod is not adopted either"
    );
    stack.clock.advance(3_000_000);
    stack.pool.reap().expect("reap");
    assert!(
        !stack.runpod.terminations().contains(&legacy),
        "an llmgw pod is never terminated here"
    );
    assert!(stack.runpod.pods().iter().any(|pod| pod.id == legacy));
}

#[test]
fn a_model_removed_from_the_registry_has_its_pod_stopped_after_restart() {
    let stack = stack();
    drop(ready(&stack));
    let restarted = restart(&stack, BTreeMap::new());
    let report = restarted.reap().expect("reap");
    assert_eq!(report.retired, vec![id("qwen-1")]);
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
    assert_eq!(
        restarted.view().totals.stop_required,
        [] as [llm_provision::Identifier; 0]
    );
}

#[test]
fn listing_through_the_provider_allocates_and_terminates_nothing() {
    let runpod = EmulatedRunpod::new();
    runpod.insert_pod(&format!("{POD_NAME_PREFIX}{ALIAS}"), BTreeMap::new());
    let mut provider = RunpodProvider::new(
        runpod.clone(),
        id("runpod"),
        id("account-1"),
        models(),
        Arc::new(ManualClock::new(0)),
    );
    let mut controller =
        Controller::new(policy(), Arc::new(LeaseRegistry::default()), 0).expect("controller");
    controller
        .apply(1, &mut provider, HostingCommand::Observe)
        .expect("observe");
    assert_eq!(runpod.lists(), 1);
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(runpod.terminations(), [] as [String; 0]);
}

#[test]
fn runpod_does_not_promise_idempotent_creates() {
    use llm_provision::HostingProvider as _;
    let provider = RunpodProvider::new(
        EmulatedRunpod::new(),
        id("runpod"),
        id("account-1"),
        models(),
        Arc::new(ManualClock::new(0)),
    );
    assert!(!provider.honours_idempotency_key());
}

// --- configuration ---------------------------------------------------------------------------

type Breakage = (&'static str, Box<dyn Fn(&mut RunpodModel)>);

#[test]
fn a_model_declaration_without_a_usable_setting_is_refused() {
    let cases: Vec<Breakage> = vec![
        ("no-gpu", Box::new(|model| model.gpu_types.clear())),
        (
            "gpu-utilization",
            Box::new(|model| model.vllm.gpu_memory_utilization = 1.5),
        ),
        (
            "gpu-utilization",
            Box::new(|model| model.vllm.gpu_memory_utilization = f64::NAN),
        ),
        (
            "zero-setting",
            Box::new(|model| model.vllm.max_model_len = 0),
        ),
        (
            "zero-setting",
            Box::new(|model| model.startup_deadline_ms = 0),
        ),
        ("zero-setting", Box::new(|model| model.idle_timeout_ms = 0)),
        (
            "zero-setting",
            Box::new(|model| model.crash_restart_limit = 0),
        ),
        ("zero-setting", Box::new(|model| model.crash_window_ms = 0)),
        (
            "secret-reference",
            Box::new(|model| model.api_key_secret = "a b}}".to_owned()),
        ),
        (
            "unsafe-text",
            Box::new(|model| model.hf_model = "bad\nmodel".to_owned()),
        ),
        (
            "unsafe-text",
            Box::new(|model| model.vllm.reasoning_effort = Some("\"high\"".to_owned())),
        ),
    ];
    for (code, break_it) in cases {
        let mut declared = model();
        break_it(&mut declared);
        assert_eq!(
            declared.validate().map_err(llm_runpod::ConfigError::code),
            Err(code),
            "{code}"
        );
        let refused = RunpodPool::new(
            policy(),
            Arc::new(LeaseRegistry::default()),
            EmulatedRunpod::new(),
            BTreeMap::from([(id(ALIAS), declared)]),
            Arc::new(ManualClock::new(0)),
        );
        assert!(refused.is_err(), "{code}");
    }
    assert_eq!(model().validate(), Ok(()));
}

#[test]
fn an_unresolved_lost_create_blocks_a_second_pod_until_it_is_resolved() {
    let stack = stack();
    // An untagged pod in the namespace means no listing can rule the lost create out.
    stack
        .runpod
        .insert_pod(&format!("{POD_NAME_PREFIX}untagged"), BTreeMap::new());
    stack.runpod.lose_next_create(false);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.clock.advance(600_001);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::StartupDeadline)
    );
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Stopping),
        "an owed stop nobody can address yet is not a reason to create a second pod"
    );
    assert_eq!(stack.runpod.create_calls(), 1);
    assert_eq!(stack.pool.view().totals.stop_required, vec![id("qwen-1")]);
}

#[test]
fn the_resource_ceiling_refuses_a_second_model_before_anything_is_submitted() {
    let runpod = EmulatedRunpod::new();
    let mut limited = policy();
    limited.max_active = 1;
    let pool = RunpodPool::new(
        limited,
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        BTreeMap::from([(id(ALIAS), model()), (id("other"), model())]),
        Arc::new(ManualClock::new(1_000)),
    )
    .expect("pool");
    assert_eq!(
        pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(
        pool.ensure(&id("other"), &authorization()).err(),
        Some(PoolError::Hosting(HostingError::CapacityExceeded))
    );
    assert_eq!(runpod.create_calls(), 1);
    let withdrawn = pool
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment == id("other-1"))
        .expect("declared");
    assert_eq!(
        withdrawn.phase,
        Phase::Cancelled,
        "a refusal before submission owes nothing"
    );
}

// --- correction round 1 ----------------------------------------------------------------------

fn ours(request: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (TAG_OWNER.to_owned(), "controller-a".to_owned()),
        (TAG_EPOCH.to_owned(), "1".to_owned()),
        (TAG_REQUEST.to_owned(), request.to_owned()),
    ])
}

/// F1's class: an identifier this adapter writes to Runpod that another create could also
/// produce. The request id is the only such value; the pod name is shared on purpose and is not
/// an identity, and the owner tag is the controller's own identity.
#[test]
fn every_create_carries_a_request_id_naming_this_controller_and_unique_to_it() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.vanish("pod-1");
    drop(ready(&stack));
    // A second pool for the same controller at the same instant: a restart whose snapshot was
    // lost. Its first create must not reuse either request id.
    let fresh = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        stack.runpod.clone(),
        models(),
        Arc::new(stack.clock.clone()),
    )
    .expect("pool");
    let _ = fresh.ensure(&id(ALIAS), &authorization());
    let ids: Vec<String> = stack
        .runpod
        .requests()
        .into_iter()
        .map(|request| request.env[TAG_REQUEST].clone())
        .collect();
    assert_eq!(ids.len(), 3);
    for (index, request) in ids.iter().enumerate() {
        assert!(
            request.starts_with("request-controller-a-qwen-"),
            "{request} does not name its controller"
        );
        assert!(
            !ids[..index].contains(request),
            "{request} was issued twice"
        );
    }
}

/// F2's class: a pod protected from the sweep by an identity weaker than its exact key. A
/// request id protects a pod only while the record it names holds no key yet.
#[test]
fn a_second_pod_carrying_a_recorded_request_id_under_another_key_is_an_orphan() {
    let stack = stack();
    drop(ready(&stack));
    let request = stack.pool.view().deployments.remove(0).request_id;
    let duplicate = stack
        .runpod
        .insert_pod(&format!("{POD_NAME_PREFIX}{ALIAS}"), ours(request.as_str()));
    let report = stack.pool.reap().expect("reap");
    assert_eq!(report.orphans, vec![id(&duplicate)]);
    assert!(
        stack.runpod.pods().iter().any(|pod| pod.id == "pod-1"),
        "the recorded pod survives"
    );
}

/// J1: restarts count inside the declared window only. Two restarts over a long life are not a
/// crash loop.
#[test]
fn restarts_spread_over_more_than_the_crash_window_are_not_a_crash_loop() {
    let stack = stack();
    stack.runpod.uptimes(vec![100, 50, 10]);
    drop(ready(&stack));
    stack.clock.advance(1_000);
    drop(
        stack
            .pool
            .ensure(&id(ALIAS), &authorization())
            .expect("one restart is not a loop"),
    );
    stack.clock.advance(600_001);
    assert!(
        stack.pool.ensure(&id(ALIAS), &authorization()).is_ok(),
        "the first restart left the window before the second one happened"
    );
    assert_eq!(stack.runpod.terminations(), [] as [String; 0]);
}

/// J1, the other side of the window: two restarts inside it still are one.
#[test]
fn restarts_inside_the_crash_window_are_a_crash_loop() {
    let stack = stack();
    stack.runpod.uptimes(vec![100, 50, 10]);
    drop(ready(&stack));
    stack.clock.advance(1_000);
    drop(
        stack
            .pool
            .ensure(&id(ALIAS), &authorization())
            .expect("one restart is not a loop"),
    );
    stack.clock.advance(600_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::CrashLoop),
        "both restarts fall inside the window, its end inclusive"
    );
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
}

/// J3: an endpoint is reported only for a running pod.
#[test]
fn a_pod_that_is_not_running_reports_no_endpoint() {
    let stack = stack();
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    stack.runpod.set_status("pod-1", PodStatus::Created);
    stack.clock.advance(1_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::Starting)
    );
    let observed = stack
        .pool
        .view()
        .deployments
        .remove(0)
        .observed
        .expect("observed");
    assert_eq!(observed.endpoint, None, "a created pod serves nothing yet");
    stack.runpod.set_status("pod-1", PodStatus::Running);
    stack.clock.advance(1_000);
    let lease = ready(&stack);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-1-8000.proxy.runpod.net")
    );
}

/// J4: in-flight accounting is per deployment.
#[test]
fn a_stream_on_a_retired_pod_does_not_keep_its_replacement_from_being_reaped() {
    let stack = stack();
    let stale = ready(&stack);
    stack.runpod.vanish("pod-1");
    drop(ready(&stack));
    stack.clock.advance(1_800_000);
    assert_eq!(
        stack.pool.reap().expect("reap").idle,
        vec![id("qwen-2")],
        "a lease on qwen-1 says nothing about traffic on qwen-2"
    );
    drop(stale);
}

/// F3 through the emulator: an exited pod still exists and is terminated.
#[test]
fn an_exited_pod_is_terminated_and_replaced() {
    let stack = stack();
    drop(ready(&stack));
    stack.runpod.set_status("pod-1", PodStatus::Exited);
    stack.clock.advance(1_000);
    assert_eq!(
        stack.pool.ensure(&id(ALIAS), &authorization()).err(),
        Some(PoolError::PodExited)
    );
    assert_eq!(stack.runpod.terminations(), vec!["pod-1".to_owned()]);
    let lease = ready(&stack);
    assert_eq!(
        lease.endpoint(),
        Some("https://pod-2-8000.proxy.runpod.net")
    );
}
