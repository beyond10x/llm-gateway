//! Correction round 2: the other side of each fix.
//!
//! F7 lets a pool stop a pod it inherited through a takeover; these cases hold that it still
//! never stops a pod any third controller holds. F8 retires a pod that stopped serving; these
//! hold that a pod whose readiness is merely unknown is not retired for it.

use std::{collections::BTreeMap, sync::Arc};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    CloudType, EmulatedRunpod, ManualClock, PoolError, RunpodModel, RunpodPool, Thinking,
    VllmSettings,
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

fn models() -> BTreeMap<Identifier, RunpodModel> {
    BTreeMap::from([(
        id(ALIAS),
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
        },
    )])
}

fn serve(pool: &RunpodPool<EmulatedRunpod>, clock: &ManualClock) {
    for _ in 0..20 {
        match pool.ensure(&id(ALIAS), &authorization()) {
            Ok(_) => return,
            Err(PoolError::Starting) => clock.advance(1_000),
            Err(other) => panic!("{}", other.code()),
        }
    }
    panic!("never served");
}

/// F7's boundary. The inherited pod's tag names the controller the snapshot came from, so it
/// may be stopped; once a third controller has retagged it, it may not, whatever its epoch.
#[test]
fn a_takeover_never_terminates_a_pod_a_third_controller_has_retagged() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let leases = Arc::new(LeaseRegistry::default());
    let first = RunpodPool::new(
        policy_for("controller-a"),
        Arc::clone(&leases),
        runpod.clone(),
        models(),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    serve(&first, &clock);
    let snapshot = first.snapshot();
    drop(first);
    clock.advance(3_600_001);
    runpod.retag("pod-1", "controller-c", 1);
    let second = RunpodPool::restore(
        policy_for("controller-b"),
        leases,
        runpod.clone(),
        models(),
        snapshot,
        Arc::new(clock.clone()),
    )
    .expect("restore");
    for _ in 0..5 {
        let _ = second.ensure(&id(ALIAS), &authorization());
        second.reap().expect("reap");
        clock.advance(1_000);
    }
    assert!(
        !runpod.terminations().contains(&"pod-1".to_owned()),
        "a pod labelled for a third controller is never terminated here"
    );
    assert!(runpod.pods().iter().any(|pod| pod.id == "pod-1"));
}

/// F8's boundary. After a pod has served, only a definite "not ready" starts the clock; a probe
/// that gets no answer says nothing about whether the pod serves.
#[test]
fn a_pod_whose_readiness_is_unknown_after_serving_is_not_retired_by_the_deadline() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy_for("controller-a"),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        models(),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    serve(&pool, &clock);
    runpod.probe_unreachable(true);
    for _ in 0..11 {
        clock.advance(60_000);
        let _ = pool.ensure(&id(ALIAS), &authorization());
        pool.reap().expect("reap");
    }
    assert!(
        runpod.terminations().is_empty(),
        "unknown readiness is not a stopped pod"
    );
}
