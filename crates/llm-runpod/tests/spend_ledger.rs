//! The pool's spend ledger (story:spend-cap-counts-earlier-pods, `spec/domains/runpod.yaml`
//! `SpendLedger`): a ceiling counts the pool's own pods' estimated spend for the day, stopped
//! ones and ones from before a restart included, until the reported spend holds at least as
//! much. Every test drives `EmulatedRunpod` and an in-memory ledger store.

use std::{collections::BTreeMap, sync::Arc};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, EmulatedRunpod, HostingPolicy, Identifier,
    LeaseRegistry, ManualClock, MemoryLedger, PoolError, RunpodModel, RunpodPool, SpendCeiling,
    SpendPolicy, SpendRefusal, Thinking, VllmSettings,
};

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
const USD: u64 = 1_000_000;

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
        max_lifetime_ms: DAY_MS,
        lease_ms: 3_600_000,
    }
}

fn authorization() -> ComputeAuthorization {
    ComputeAuthorization {
        ledger: id("ledger-1"),
        reservation: id("spend-ceiling"),
    }
}

/// A model whose startup deadline is half an hour and whose idle window is one minute.
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
        startup_deadline_ms: 1_800_000,
        idle_timeout_ms: 60_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "default".to_owned(),
        fallbacks: Vec::new(),
    }
}

/// Every alias costs 2 USD an hour, so its startup estimate is 1 USD.
fn spend(aliases: &[&str], daily: Option<u64>, total: Option<u64>) -> SpendPolicy {
    SpendPolicy {
        daily_cap_micro_usd: daily,
        total_cap_micro_usd: total,
        price_per_hour_micro_usd: aliases.iter().map(|alias| (id(alias), 2 * USD)).collect(),
    }
}

fn reported(daily_micro_usd: u64, total_micro_usd: u64) -> AccountSpend {
    AccountSpend {
        daily_micro_usd,
        total_micro_usd,
    }
}

struct World {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    ledger: MemoryLedger,
}

impl World {
    /// The first instant of a UTC day, plus one second.
    fn new() -> Self {
        let runpod = EmulatedRunpod::new();
        runpod.set_account_spend(Some(reported(0, 0)));
        Self {
            runpod,
            clock: ManualClock::new(20_000 * DAY_MS + 1_000),
            ledger: MemoryLedger::new(),
        }
    }

    /// A pool as the gateway builds it at start: fresh leases, this world's control plane,
    /// clock and ledger store.
    fn pool(&self, aliases: &[&str], spend: SpendPolicy) -> RunpodPool<EmulatedRunpod> {
        RunpodPool::new(
            policy(),
            Arc::new(LeaseRegistry::default()),
            self.runpod.clone(),
            aliases
                .iter()
                .map(|alias| (id(alias), model()))
                .collect::<BTreeMap<_, _>>(),
            Arc::new(self.clock.clone()),
        )
        .expect("pool")
        .with_spend(spend)
        .with_ledger(Box::new(self.ledger.clone()))
    }
}

/// Starts `alias`'s pod and takes it ready: its create is submitted now.
fn run(pool: &RunpodPool<EmulatedRunpod>, alias: &str) {
    assert_eq!(
        pool.ensure(&id(alias), &authorization()).err(),
        Some(PoolError::Starting)
    );
    drop(
        pool.ensure(&id(alias), &authorization())
            .expect("ready on its first probe"),
    );
}

fn over(ceiling: SpendCeiling, micro_usd: u64) -> PoolError {
    PoolError::SpendCeiling(SpendRefusal::Over {
        ceiling,
        over_micro_usd: micro_usd,
    })
}

#[test]
fn a_stopped_pod_counts_until_the_reported_spend_holds_at_least_as_much() {
    let world = World::new();
    let pool = world.pool(&["qwen"], spend(&["qwen"], Some(10 * USD), None));
    run(&pool, "qwen");
    world.clock.advance(5 * HOUR_MS);
    pool.reap().expect("cleanup");
    assert_eq!(world.runpod.terminations(), ["pod1"]);
    // Five hours at 2 USD: 10 in the ledger, none reported yet, 1 for the start: 11.
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Daily, USD)),
        "the stopped pod still counts"
    );
    assert_eq!(world.runpod.create_calls(), 1);
    // Billing has caught up with part of it: the larger amount counts, not the sum.
    world
        .runpod
        .set_account_spend(Some(reported(8 * USD, 8 * USD)));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Daily, USD))
    );
    // Billing reports more than the ledger: the reported spend counts, once.
    world
        .runpod
        .set_account_spend(Some(reported(10 * USD + 500_000, 10 * USD + 500_000)));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Daily, 1_500_000))
    );
    // The next UTC day the stopped pod's spend is no longer the day's.
    world.runpod.set_account_spend(Some(reported(0, 10 * USD)));
    world.clock.advance(DAY_MS);
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(world.runpod.create_calls(), 2);
}

#[test]
fn the_total_ceiling_counts_the_ledger_over_the_reported_window() {
    let world = World::new();
    let pool = world.pool(&["qwen"], spend(&["qwen"], None, Some(10 * USD + 500_000)));
    run(&pool, "qwen");
    world.clock.advance(5 * HOUR_MS);
    pool.reap().expect("cleanup");
    world.clock.advance(DAY_MS);
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Total, 500_000)),
        "yesterday's 10 USD and 1 for the start"
    );
    world.clock.advance(30 * DAY_MS);
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::Starting),
        "outside the 30 days the total counts"
    );
}

#[test]
fn a_restarted_pool_reads_the_ledger_and_refuses_a_start_it_puts_over_the_ceiling() {
    let world = World::new();
    let first = world.pool(&["qwen"], spend(&["qwen"], Some(10 * USD), None));
    run(&first, "qwen");
    world.clock.advance(5 * HOUR_MS);
    // The gateway stops without stopping its pod, and starts again.
    drop(first);
    let second = world.pool(&["qwen"], spend(&["qwen"], Some(10 * USD), None));
    assert_eq!(
        second.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Daily, USD)),
        "the pod from before the restart has run five hours"
    );
    assert_eq!(world.runpod.create_calls(), 1);
    // The orphan sweep stops it: its stop is recorded, and it still counts.
    let report = second.reap().expect("cleanup");
    assert_eq!(report.orphans, [id("pod1")]);
    world.clock.advance(HOUR_MS);
    assert_eq!(
        second.ensure(&id("qwen"), &authorization()).err(),
        Some(over(SpendCeiling::Daily, USD)),
        "stopped at the sweep: it counts five hours, not six"
    );
    let entries = world.ledger.entries().expect("a ledger");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].pod.as_deref(), Some("pod1"));
    assert_eq!(
        entries[0]
            .stopped_ms
            .map(|stopped| stopped - entries[0].started_ms),
        Some(5 * HOUR_MS)
    );
    assert_eq!(entries[0].price_per_hour_micro_usd, 2 * USD);
}

#[test]
fn a_live_pod_counts_once() {
    let world = World::new();
    let pool = world.pool(&["a", "b"], spend(&["a", "b"], Some(10 * USD), None));
    run(&pool, "a");
    world.clock.advance(4 * HOUR_MS);
    // a has run 4 hours: 8 USD, counted by the running estimate and not again by the ledger;
    // b's start 1: 9.
    assert_eq!(
        pool.ensure(&id("b"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(world.runpod.create_calls(), 2);
}

#[test]
fn a_ledger_that_cannot_be_read_refuses_every_start_as_unknown() {
    let world = World::new();
    world.ledger.set_text(Some("not a ledger".to_owned()));
    let pool = world.pool(&["qwen"], spend(&["qwen"], Some(10 * USD), None));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Unknown))
    );
    assert_eq!(world.runpod.create_calls(), 0);
    assert_eq!(
        world.ledger.text().as_deref(),
        Some("not a ledger"),
        "an unreadable ledger is never written over"
    );

    let world = World::new();
    world.ledger.fail_reads(true);
    let pool = world.pool(&["qwen"], spend(&["qwen"], Some(10 * USD), None));
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Unknown))
    );
    assert_eq!(world.runpod.create_calls(), 0);
}

#[test]
fn a_ledger_write_that_fails_refuses_the_next_start_as_unknown() {
    let world = World::new();
    let pool = world.pool(&["a", "b"], spend(&["a", "b"], Some(10 * USD), None));
    world.ledger.fail_writes(true);
    assert_eq!(
        pool.ensure(&id("a"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(
        pool.ensure(&id("b"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Unknown)),
        "the ledger no longer holds a's start"
    );
    assert_eq!(world.runpod.create_calls(), 1);
}
