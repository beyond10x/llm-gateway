//! Adversary cases for story:cold-start-hold, pass 2: `RunpodPool::ensure_held` when the stop
//! of the pod a hold waited on is not confirmed. The emulator terminates at once in every other
//! test, so the record is terminal and has left the slot by the next ask; these cases keep it
//! `stop-required` in the slot (the pod vanished, and every listing is partial), which is the
//! only state the hold's "owes a stop" rule decides. In-process transport; nothing is opened.

use std::{collections::BTreeMap, sync::Arc};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry, Phase};
use llm_runpod::{
    CloudType, EmulatedRunpod, Hold, ManualClock, PoolError, RunpodModel, RunpodPool, Thinking,
    VllmSettings,
};

const ALIAS: &str = "qwen";
const DEADLINE_MS: u64 = 600_000;

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
            max_model_len: 65_536,
            max_num_seqs: 8,
            gpu_memory_utilization: 0.9,
            thinking: Thinking::Off,
            reasoning_effort: None,
            sampling: None,
            extra_args: Vec::new(),
        },
        api_key_secret: "qwen_vllm_key".to_owned(),
        startup_deadline_ms: DEADLINE_MS,
        idle_timeout_ms: 1_800_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

struct Stack {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    pool: RunpodPool<EmulatedRunpod>,
}

fn stack() -> Stack {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        BTreeMap::from([(id(ALIAS), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    Stack {
        runpod,
        clock,
        pool,
    }
}

fn ask(stack: &Stack, hold: &mut Hold) -> Result<(), PoolError> {
    stack
        .pool
        .ensure_held(&id(ALIAS), &authorization(), hold)
        .map(drop)
}

fn phase(stack: &Stack, deployment: &str) -> Phase {
    stack
        .pool
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment.as_str() == deployment)
        .expect("record")
        .phase
}

/// Two holds on `qwen-1`, then `qwen-1` misses its startup deadline and its stop cannot be
/// confirmed. Returns the stack, the hold whose step retired it, and the other one.
fn two_holds_on_an_unconfirmed_stop() -> (Stack, Hold, Hold) {
    let stack = stack();
    stack.runpod.ready_after(u32::MAX);
    let (mut first, mut second) = (Hold::default(), Hold::default());
    assert_eq!(ask(&stack, &mut first), Err(PoolError::Starting));
    assert_eq!(ask(&stack, &mut second), Err(PoolError::Starting));
    assert_eq!(second.deployment(), Some(&id("qwen-1")));
    // The pod disappears out of band and no listing can say so: the terminate is refused and
    // nothing ever confirms the stop.
    stack.runpod.vanish("pod1");
    stack.runpod.partial_listing(true);
    stack.clock.advance(DEADLINE_MS + 1);
    assert_eq!(ask(&stack, &mut first), Err(PoolError::StartupDeadline));
    assert!(first.lost());
    assert_eq!(phase(&stack, "qwen-1"), Phase::StopRequired);
    (stack, first, second)
}

/// runpod.yaml: "Once its bound deployment owes a stop, is retired, or is no longer the slot's
/// current one, the hold is lost". With the stop unconfirmed the deployment is still the slot's
/// current one and was not retired by the second hold's step; only "owes a stop" decides it.
#[test]
fn adv2_l6_a_hold_whose_pod_owes_an_unconfirmed_stop_is_lost() {
    let (stack, _first, mut second) = two_holds_on_an_unconfirmed_stop();
    stack.clock.advance(500);
    assert_eq!(ask(&stack, &mut second), Err(PoolError::Stopping));
    assert!(
        second.lost(),
        "the second hold read an owed stop as still waiting"
    );
    assert_eq!(stack.runpod.create_calls(), 1);
}

/// runpod.yaml / deployment.yaml: a caller that arrives while the slot is stopping a previous
/// deployment waits unbound, and binds to the replacement once the stop is confirmed.
#[test]
fn adv2_l6_a_hold_that_arrives_during_an_unconfirmed_stop_waits_unbound_then_binds_to_the_replacement()
 {
    let (stack, _first, _second) = two_holds_on_an_unconfirmed_stop();
    let mut late = Hold::default();
    for _ in 0..3 {
        stack.clock.advance(500);
        assert_eq!(ask(&stack, &mut late), Err(PoolError::Stopping));
        assert!(!late.lost(), "a hold that never waited on qwen-1 was lost");
        assert_eq!(late.deployment(), None, "bound to the pod being stopped");
    }
    assert_eq!(
        stack.runpod.create_calls(),
        1,
        "a create during an owed stop"
    );
    // A complete listing without the pod confirms the stop; the next ask starts the replacement.
    stack.runpod.partial_listing(false);
    stack.runpod.ready_after(0);
    stack.clock.advance(500);
    assert_eq!(ask(&stack, &mut late), Err(PoolError::Starting));
    assert_eq!(late.deployment(), Some(&id("qwen-2")));
    assert_eq!(stack.runpod.create_calls(), 2);
    stack.clock.advance(500);
    assert_eq!(ask(&stack, &mut late), Ok(()));
    assert!(!late.lost());
}

/// A create whose answer was lost and which allocated nothing is discharged by the next
/// listing (`a_lost_create_that_allocated_nothing_is_discharged_and_the_next_request_starts_afresh`).
/// No pod ever existed, yet a hold bound to that deployment is lost by it, as the rules say
/// for a deployment that left the slot: the caller is refused rather than served by the
/// replacement its own next ask could have started.
#[test]
fn adv2_l6_a_hold_on_a_lost_create_that_allocated_nothing_is_lost() {
    let stack = stack();
    stack.runpod.lose_next_create(false);
    let mut hold = Hold::default();
    assert_eq!(ask(&stack, &mut hold), Err(PoolError::Starting));
    assert_eq!(hold.deployment(), Some(&id("qwen-1")));
    stack.clock.advance(500);
    assert_eq!(ask(&stack, &mut hold), Err(PoolError::Stopping));
    assert!(hold.lost());
    assert_eq!(stack.runpod.create_calls(), 1);
    assert!(stack.runpod.pods().is_empty(), "no pod was ever made");
}
