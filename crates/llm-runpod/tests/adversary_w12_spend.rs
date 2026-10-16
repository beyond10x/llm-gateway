//! Adversarial pass over story:spend-caps: behaviour of the spend check before each create.
//! Every test drives `EmulatedRunpod`; nothing leaves the process.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, CreateAnswer, EmulatedRunpod, HostingPolicy,
    Identifier, LeaseRegistry, ManualClock, PodListing, PodRequest, PoolError, Probe, ProbeTarget,
    RunpodModel, RunpodPool, RunpodTransport, SpendCeiling, SpendPolicy, SpendRefusal,
    TerminateAnswer, Thinking, VllmSettings,
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
        idle_timeout_ms: 86_400_000,
        crash_restart_limit: 2,
        crash_window_ms: 600_000,
    }
}

fn models(aliases: &[&str]) -> BTreeMap<Identifier, RunpodModel> {
    aliases.iter().map(|alias| (id(alias), model())).collect()
}

/// A daily ceiling of 10 USD; every alias costs 2 USD an hour, so a start is estimated at 1 USD.
fn spend(aliases: &[&str]) -> SpendPolicy {
    SpendPolicy {
        daily_cap_micro_usd: Some(10_000_000),
        total_cap_micro_usd: None,
        price_per_hour_micro_usd: aliases.iter().map(|alias| (id(alias), 2_000_000)).collect(),
    }
}

fn reported(daily_micro_usd: u64) -> AccountSpend {
    AccountSpend {
        daily_micro_usd,
        total_micro_usd: daily_micro_usd,
    }
}

/// Two starts back to back (two models' first requests arriving together, which the pool's lock
/// serialises into exactly this) must not both be admitted against the same headroom. Reported
/// 8.5 USD, ceiling 10, each start estimated at 1 USD over its startup deadline: the first
/// projects 9.5 and is created. The second must count the first pod, which will run at least
/// its startup too: 8.5 + 1 + 1 = 10.5, 0.5 past the ceiling. The check counts the first pod's
/// running estimate from its submission, which is 0 ms ago, so it projects 9.5 again and admits.
#[test]
fn adversary_w12_two_starts_are_not_both_admitted_against_one_headroom() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        runpod.clone(),
        models(&["a", "b"]),
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(spend(&["a", "b"]));
    runpod.set_account_spend(Some(reported(8_500_000)));
    assert_eq!(
        pool.ensure(&id("a"), &authorization()).err(),
        Some(PoolError::Starting),
        "the first start fits: 8.5 + 1 = 9.5"
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(
        pool.ensure(&id("b"), &authorization()).err(),
        Some(PoolError::SpendCeiling(SpendRefusal::Over {
            ceiling: SpendCeiling::Daily,
            over_micro_usd: 500_000,
        })),
        "the second start was admitted against the headroom the first already took: \
         create_calls = {}",
        runpod.create_calls()
    );
    assert_eq!(runpod.create_calls(), 1);
}

/// A transport that records the day start each spend read is asked for.
#[derive(Clone, Default)]
struct DayStarts {
    runpod: EmulatedRunpod,
    asked: Arc<Mutex<Vec<u64>>>,
}

impl RunpodTransport for DayStarts {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.runpod.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
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
        self.asked.lock().unwrap().push(day_start_ms);
        self.runpod.account_spend(day_start_ms)
    }
}

/// The daily ceiling is only as right as the day start the provider asks the transport for, and
/// `EmulatedRunpod` ignores it, so no conformance scenario observes it. 2026-10-16T13:45:00Z
/// must ask from 2026-10-16T00:00:00Z, and one millisecond before midnight from the same day.
#[test]
fn adversary_w12_the_spend_read_is_asked_from_the_start_of_the_utc_day() {
    const DAY_START: u64 = 1_792_108_800_000; // 2026-10-16T00:00:00Z
    let transport = DayStarts::default();
    transport.runpod.set_account_spend(Some(reported(0)));
    let clock = ManualClock::new(DAY_START + 49_500_000); // 13:45:00
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport.clone(),
        models(&["a", "b"]),
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(SpendPolicy {
        daily_cap_micro_usd: Some(1_000_000_000),
        ..spend(&["a", "b"])
    });
    assert_eq!(
        pool.ensure(&id("a"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    clock.advance(86_400_000 - 49_500_000 - 1); // 23:59:59.999
    assert_eq!(
        pool.ensure(&id("b"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    assert_eq!(*transport.asked.lock().unwrap(), [DAY_START, DAY_START]);
}
