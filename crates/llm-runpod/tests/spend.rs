//! The spend check (story:spend-caps, `spec/domains/runpod.yaml` `SpendLedger`): a pod start
//! that would pass a spend ceiling of the pool's account is refused before any create. The
//! conformance scenarios cover under, at and over each ceiling; these cover what they do not:
//! the refusal's text, the conversion of declared amounts, a refused start that later fits, a
//! restored pool's live pods and a model without a price. Every test drives `EmulatedRunpod`.

use std::{collections::BTreeMap, sync::Arc};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, EmulatedRunpod, HostingPolicy, Identifier,
    LeaseRegistry, ManualClock, PoolError, RunpodModel, RunpodPool, SpendCeiling, SpendPolicy,
    SpendRefusal, Thinking, VllmSettings, micro_usd,
};

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
        reservation: id("spend-ceiling"),
    }
}

/// A model whose startup deadline is half an hour.
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
        startup_deadline_ms: 1_800_000,
        idle_timeout_ms: 86_400_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "default".to_owned(),
        fallbacks: Vec::new(),
    }
}

fn models(aliases: &[&str]) -> BTreeMap<Identifier, RunpodModel> {
    aliases.iter().map(|alias| (id(alias), model())).collect()
}

/// A daily ceiling of 10 USD; every alias in `priced` costs 2 USD an hour.
fn spend(priced: &[&str]) -> SpendPolicy {
    SpendPolicy {
        daily_cap_micro_usd: Some(10_000_000),
        total_cap_micro_usd: None,
        price_per_hour_micro_usd: priced.iter().map(|alias| (id(alias), 2_000_000)).collect(),
    }
}

fn reported(daily_micro_usd: u64) -> AccountSpend {
    AccountSpend {
        daily_micro_usd,
        total_micro_usd: daily_micro_usd,
    }
}

fn pool(
    runpod: &EmulatedRunpod,
    clock: &ManualClock,
    leases: &Arc<LeaseRegistry>,
    aliases: &[&str],
) -> RunpodPool<EmulatedRunpod> {
    RunpodPool::new(
        policy(),
        Arc::clone(leases),
        runpod.clone(),
        models(aliases),
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(spend(aliases))
}

#[test]
fn the_spend_refusal_says_which_ceiling_and_by_how_much() {
    let daily = SpendRefusal::Over {
        ceiling: SpendCeiling::Daily,
        over_micro_usd: 500_000,
    };
    assert_eq!(
        daily.to_string(),
        "the daily spend ceiling would be passed by 0.500000 USD"
    );
    assert_eq!(daily.code(), "daily");
    let total = SpendRefusal::Over {
        ceiling: SpendCeiling::Total,
        over_micro_usd: 12_345_678,
    };
    assert_eq!(
        total.to_string(),
        "the total spend ceiling would be passed by 12.345678 USD"
    );
    assert_eq!(SpendRefusal::Unknown.code(), "unknown");
    assert_eq!(
        SpendRefusal::Unknown.to_string(),
        "the account's spend is unknown, so no ceiling is assumed met"
    );
    assert_eq!(PoolError::SpendCeiling(daily).code(), "spend-ceiling");
}

#[test]
fn micro_usd_takes_only_a_positive_finite_amount_of_at_least_one_micro_usd() {
    assert_eq!(micro_usd(2.5), Some(2_500_000));
    assert_eq!(micro_usd(0.000_001), Some(1));
    assert_eq!(micro_usd(1_000_000.0), Some(1_000_000_000_000));
    for refused in [
        0.0,
        -0.0,
        -1.0,
        0.000_000_4,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ] {
        assert_eq!(micro_usd(refused), None, "{refused}");
    }
}

#[test]
fn a_refused_start_sends_nothing_and_a_later_one_that_fits_starts() {
    let (runpod, clock, leases) = (
        EmulatedRunpod::new(),
        ManualClock::new(1_000),
        Arc::new(LeaseRegistry::default()),
    );
    let pool = pool(&runpod, &clock, &leases, &["qwen"]);
    runpod.set_account_spend(Some(reported(9_000_001)));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Over {
            ceiling: SpendCeiling::Daily,
            over_micro_usd: 1,
        })),
        "one micro-USD past the ceiling is refused"
    );
    assert_eq!(runpod.create_calls(), 0);
    runpod.set_account_spend(Some(reported(9_000_000)));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(runpod.spend_reads(), 2);
}

#[test]
fn a_restored_pool_counts_the_live_pods_its_records_hold() {
    let (runpod, clock, leases) = (
        EmulatedRunpod::new(),
        ManualClock::new(1_000),
        Arc::new(LeaseRegistry::default()),
    );
    runpod.set_account_spend(Some(reported(0)));
    let first = pool(&runpod, &clock, &leases, &["a", "b"]);
    assert_eq!(
        first.ensure(&id("a"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    let restored = RunpodPool::restore(
        policy(),
        Arc::clone(&leases),
        runpod.clone(),
        models(&["a", "b"]),
        first.snapshot(),
        Arc::new(clock.clone()),
    )
    .expect("restore")
    .with_spend(spend(&["a", "b"]));
    // a: 2 USD/h for half an hour = 1; b about to start: 1; reported 8.5: 10.5 > 10. Without
    // the restored record's running estimate it would be 9.5, and admitted.
    clock.advance(1_800_000);
    runpod.set_account_spend(Some(reported(8_500_000)));
    assert_eq!(
        restored.ensure(&id("b"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Over {
            ceiling: SpendCeiling::Daily,
            over_micro_usd: 500_000,
        }))
    );
    assert_eq!(runpod.create_calls(), 1);
}

#[test]
fn a_model_without_a_declared_price_is_never_started_under_a_ceiling() {
    let (runpod, clock, leases) = (
        EmulatedRunpod::new(),
        ManualClock::new(1_000),
        Arc::new(LeaseRegistry::default()),
    );
    let pool = RunpodPool::new(
        policy(),
        Arc::clone(&leases),
        runpod.clone(),
        models(&["qwen"]),
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(spend(&[]));
    runpod.set_account_spend(Some(reported(0)));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Unknown))
    );
    assert_eq!(runpod.create_calls(), 0);
}

#[test]
fn a_spend_read_that_answers_nothing_is_never_a_met_ceiling() {
    let (runpod, clock, leases) = (
        EmulatedRunpod::new(),
        ManualClock::new(1_000),
        Arc::new(LeaseRegistry::default()),
    );
    let pool = pool(&runpod, &clock, &leases, &["qwen"]);
    runpod.set_account_spend(Some(reported(0)));
    runpod.set_account_spend(None);
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Unknown))
    );
    assert_eq!(runpod.create_calls(), 0);
    assert_eq!(runpod.spend_reads(), 1);
}
