//! Adversarial pass 2 over story:spend-caps: the floor a live pod counts at. Every test drives
//! `EmulatedRunpod`; nothing leaves the process.

use std::{collections::BTreeMap, sync::Arc};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, EmulatedRunpod, HostingPolicy, Identifier,
    LeaseRegistry, ManualClock, PoolError, RunpodModel, RunpodPool, SpendCeiling, SpendPolicy,
    SpendRefusal, Thinking, VllmSettings,
};

fn id(value: &str) -> Identifier {
    Identifier::new(value).unwrap()
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
        reservation: id("spend-ceiling"),
    }
}

fn model(startup_deadline_ms: u64) -> RunpodModel {
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
        startup_deadline_ms,
        idle_timeout_ms: 86_400_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

/// Every spend case before this one declares the same startup deadline for every model, so the
/// floor a live pod counts at (`provider.rs`, `admit_spend`: the larger of its running estimate
/// and its price times *its own* startup deadline) is never told apart from the startup
/// deadline of the model about to start. Here they differ: `slow` starts in an hour (2 USD at
/// 2 USD an hour), `quick` in six minutes (0.2 USD). Reported 7.9, daily ceiling 10: `slow`
/// projects 9.9 and is created; `quick` at once must count `slow` at its own 2 USD,
/// 7.9 + 2 + 0.2 = 10.1, 0.1 past the ceiling. Counting the live pod at the starting model's
/// deadline would project 8.3 and admit it.
#[test]
fn adversary_w12p2_a_live_pod_counts_at_its_own_startup_deadline() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let models: BTreeMap<_, _> = [
        (id("slow"), model(3_600_000)),
        (id("quick"), model(360_000)),
    ]
    .into_iter()
    .collect();
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        models,
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(SpendPolicy {
        daily_cap_micro_usd: Some(10_000_000),
        total_cap_micro_usd: None,
        price_per_hour_micro_usd: [(id("slow"), 2_000_000), (id("quick"), 2_000_000)]
            .into_iter()
            .collect(),
    });
    runpod.set_account_spend(Some(AccountSpend {
        daily_micro_usd: 7_900_000,
        total_micro_usd: 7_900_000,
    }));
    assert_eq!(
        pool.ensure(&id("slow"), &authorization()).err(),
        Some(PoolError::Starting),
        "the first start fits: 7.9 + 2 = 9.9"
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(
        pool.ensure(&id("quick"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Over {
            ceiling: SpendCeiling::Daily,
            over_micro_usd: 100_000,
        })),
        "the live pod was not counted at its own startup estimate: create_calls = {}",
        runpod.create_calls()
    );
    assert_eq!(runpod.create_calls(), 1);
}
