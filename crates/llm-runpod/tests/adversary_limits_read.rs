//! Adversary pass on the owner's read of the spend ceilings (story:limits-read):
//! `RunpodPool::spend_ceilings` against the claims `docs/gateway.md` ("The spend ceilings") and
//! `docs/hosting.md` make of it: the read counts "the same values a start's check reads", so "a
//! start is admitted exactly when its own estimate is at most `left_usd` of every entry", and the
//! daily row counts the day's ledger spend plus each live pod's whole run so far. Every test
//! drives `EmulatedRunpod`, a `ManualClock` and an in-memory ledger store.

use std::{collections::BTreeMap, sync::Arc};

use llm_runpod::{
    AccountSpend, CeilingReading, CloudType, ComputeAuthorization, EmulatedRunpod, HostingPolicy,
    Identifier, LeaseRegistry, ManualClock, MemoryLedger, PoolError, RunpodModel, RunpodPool,
    SpendCeiling, SpendPolicy, Thinking, VllmSettings,
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

/// A model whose startup deadline is half an hour (at 2 USD an hour its startup estimate is
/// 1 USD) and whose idle window is a day, so no idle stop interferes.
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
        idle_timeout_ms: DAY_MS,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
        profile: "default".to_owned(),
        fallbacks: Vec::new(),
    }
}

struct World {
    runpod: EmulatedRunpod,
    clock: ManualClock,
    ledger: MemoryLedger,
}

impl World {
    fn new(start_ms: u64, reported_daily: u64) -> Self {
        let runpod = EmulatedRunpod::new();
        runpod.set_account_spend(Some(AccountSpend {
            daily_micro_usd: reported_daily,
            total_micro_usd: reported_daily,
        }));
        Self {
            runpod,
            clock: ManualClock::new(start_ms),
            ledger: MemoryLedger::new(),
        }
    }

    /// A pool as the gateway builds it: a daily ceiling of `daily_cap`, every alias at 2 USD an
    /// hour, and a ledger.
    fn pool(&self, aliases: &[&str], daily_cap: u64) -> RunpodPool<EmulatedRunpod> {
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
        .with_spend(SpendPolicy {
            daily_cap_micro_usd: Some(daily_cap),
            total_cap_micro_usd: None,
            price_per_hour_micro_usd: aliases.iter().map(|alias| (id(alias), 2 * USD)).collect(),
        })
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

fn daily(pool: &RunpodPool<EmulatedRunpod>) -> CeilingReading {
    pool.spend_ceilings()
        .into_iter()
        .find(|reading| reading.ceiling == SpendCeiling::Daily)
        .expect("a daily ceiling is declared")
}

/// The read says a 1 USD start does not fit (left 0.5 USD) at the very instant the check admits
/// one. a's pod ended out of band (Runpod lost the host, or it was terminated elsewhere) a minute
/// after its create; the pool has not listed since. The read counts a's record as a live pod at
/// its 1 USD startup floor without the listing a start's check makes first (`hand_out` steps and
/// observes before `set_live_spend`; `spend_ceilings` only calls `set_live_spend`), so it counts
/// values the start's check does not.
#[test]
fn adv_limits_the_read_and_the_next_start_agree_after_a_pod_ended_out_of_band() {
    let world = World::new(20_000 * DAY_MS + HOUR_MS, 8_500_000);
    let pool = world.pool(&["a", "b"], 10 * USD);
    run(&pool, "a");
    world.clock.advance(60_000);
    world.runpod.vanish("pod1");
    let read = daily(&pool);
    let left = read
        .spent_micro_usd
        .map(|spent| (10 * USD).saturating_sub(spent))
        .expect("the spend is counted");
    let start = pool.ensure(&id("b"), &authorization()).err();
    let admitted = start == Some(PoolError::Starting);
    // b's own estimate: 2 USD an hour over its half-hour startup deadline.
    let estimate = USD;
    assert_eq!(
        admitted,
        estimate <= left,
        "the read left {left} micro-USD (spent {:?}) for a {estimate} micro-USD start, and the \
         start at the same instant answered {start:?}",
        read.spent_micro_usd
    );
}

/// The daily window is the current UTC day, and the daily row counts what the check counts
/// against the daily ceiling (docs/gateway.md, "The spend ceilings"; docs/hosting.md): the day's
/// part of the ledger plus each live pod's whole run so far, the part before the day began
/// included. a's pod was submitted at 23:00 and is still running at 01:00: both hours (4 USD)
/// count against today's ceiling.
#[test]
fn adv_limits_daily_spent_counts_a_live_pods_whole_run_so_far() {
    let day = 20_000 * DAY_MS;
    let world = World::new(day + 23 * HOUR_MS, 0);
    let pool = world.pool(&["a"], 10 * USD);
    run(&pool, "a");
    // A read every half hour, as the running gateway's cleanup pass steps the pool every
    // minute: each step renews the pool's one-hour ownership lease, so the pod stays live.
    let mut read = daily(&pool);
    for _ in 0..4 {
        world.clock.advance(HOUR_MS / 2);
        read = daily(&pool);
    }
    assert_eq!(
        (read.window_start_ms, read.window_end_ms),
        (day + DAY_MS, day + 2 * DAY_MS)
    );
    assert_eq!(
        read.spent_micro_usd,
        Some(4 * USD),
        "both hours at 2 USD, the one before the window [{}, {}) included",
        read.window_start_ms,
        read.window_end_ms
    );
}

/// No test of the change reads with a live pod. The read must count a live pod as a start's
/// check does: at least its startup estimate right after its create, and its running estimate
/// later, not again through the ledger. Kills the mutant that drops `set_live_spend` from
/// `spend_ceilings` (the read then counts a stale live list: none after one start, and a's
/// ledger entry from its submission instead of its 1 USD floor).
#[test]
fn adv_limits_the_read_counts_a_live_pod_as_the_check_does() {
    let world = World::new(20_000 * DAY_MS + HOUR_MS, 0);
    let pool = world.pool(&["a", "b"], 10 * USD);
    run(&pool, "a");
    assert_eq!(daily(&pool).spent_micro_usd, Some(USD), "the startup floor");
    world.clock.advance(4 * HOUR_MS);
    assert_eq!(
        daily(&pool).spent_micro_usd,
        Some(8 * USD),
        "four hours at 2 USD, counted once"
    );
    assert_eq!(world.runpod.create_calls(), 1, "the read starts nothing");
}
