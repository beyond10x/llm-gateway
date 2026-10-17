//! Security review of story:spend-caps: the spend ceiling is a spending boundary.
//!
//! Integrity checks of the change's own contract (`spec/domains/runpod.yaml`, `SpendLedger`;
//! `docs/hosting.md`, "Spend ceilings"): every create the provider sends is immediately preceded
//! by a spend read that admitted it, on every create path the pool has, and a spend answer that
//! is not wholly a success is never taken for spend. Every test drives `EmulatedRunpod` or the
//! `connectors-fixture` binary; nothing leaves the loopback interface.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use llm_runpod::{
    AccountSpend, CloudType, ComputeAuthorization, ConnectorsBinding, ConnectorsRunpod,
    CreateAnswer, EmulatedRunpod, HostingPolicy, Identifier, LeaseRegistry, ManualClock,
    PodListing, PodRequest, PodStatus, PoolError, Probe, ProbeTarget, RunpodModel, RunpodPool,
    RunpodTransport, SpendPolicy, TerminateAnswer, Thinking, VllmSettings, started_at_ms,
};
use serde_json::{Value, json};

fn id(value: &str) -> Identifier {
    Identifier::new(value).expect("test identifier")
}

/// What the transport was asked, in order: `spend`, `create` and nothing else is logged.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<&'static str>>>);

impl Log {
    fn push(&self, event: &'static str) {
        self.0.lock().expect("log").push(event);
    }
    fn events(&self) -> Vec<&'static str> {
        self.0.lock().expect("log").clone()
    }
}

/// `EmulatedRunpod`, with every spend read and every create logged in order.
struct Recorder {
    inner: EmulatedRunpod,
    log: Log,
}

impl RunpodTransport for Recorder {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        self.inner.list_pods()
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        self.log.push("create");
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
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        self.log.push("spend");
        self.inner.account_spend(day_start_ms)
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

fn authorization() -> ComputeAuthorization {
    ComputeAuthorization {
        ledger: id("ledger-1"),
        reservation: id("spend-ceiling"),
    }
}

/// Invariant 1, at step 4 of the ladder: on the pool's create paths (the first start, its GPU
/// fallback after a placement refusal, and the replacement of a pod that exited) every create is
/// immediately preceded by a spend read, and no create is sent without one.
#[test]
fn sec_every_create_on_every_start_path_is_immediately_preceded_by_a_spend_read() {
    let runpod = EmulatedRunpod::new();
    let clock = ManualClock::new(1_000);
    let log = Log::default();
    let pool = RunpodPool::new(
        HostingPolicy {
            controller: id("controller-a"),
            provider: id("runpod"),
            account: id("account-1"),
            ledger: id("ledger-1"),
            max_active: 16,
            max_lifetime_ms: 86_400_000,
            lease_ms: 3_600_000,
        },
        Arc::new(LeaseRegistry::default()),
        Recorder {
            inner: runpod.clone(),
            log: log.clone(),
        },
        BTreeMap::from([(id("qwen"), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool")
    .with_spend(SpendPolicy {
        daily_cap_micro_usd: Some(1_000_000_000),
        total_cap_micro_usd: Some(1_000_000_000),
        price_per_hour_micro_usd: BTreeMap::from([(id("qwen"), 2_000_000)]),
    });
    runpod.set_account_spend(Some(AccountSpend {
        daily_micro_usd: 0,
        total_micro_usd: 0,
    }));
    // The first GPU type is refused, so the first start walks the fallback.
    runpod.refuse_gpu("NVIDIA L40S");
    assert_eq!(
        pool.ensure(&id("qwen"), &authorization()).err(),
        Some(PoolError::Starting)
    );
    // The pod exits; the pool terminates it and starts a replacement.
    runpod.set_status("pod1", PodStatus::Exited);
    for _ in 0..10 {
        if runpod.create_calls() >= 3 {
            break;
        }
        clock.advance(1_000);
        let _ = pool.ensure(&id("qwen"), &authorization());
    }
    assert!(
        runpod.create_calls() >= 3,
        "the paths under test were not all reached: {} creates, {:?}",
        runpod.create_calls(),
        log.events()
    );
    let events = log.events();
    for (at, event) in events.iter().enumerate() {
        if *event == "create" {
            assert!(
                at > 0 && events[at - 1] == "spend",
                "create #{at} was not immediately preceded by a spend read: {events:?}"
            );
        }
    }
}

/// One fixture of the `connectors` CLI under cargo's temporary directory.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("security_w12_spend")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("work")).expect("fixture dir");
        std::os::unix::fs::symlink(
            env!("CARGO_BIN_EXE_connectors-fixture"),
            dir.join("connectors"),
        )
        .expect("fixture link");
        Self { dir }
    }

    fn script(&self, script: &Value) {
        fs::write(self.dir.join("script.json"), script.to_string()).expect("script");
    }

    fn transport(&self) -> ConnectorsRunpod {
        ConnectorsRunpod::new(ConnectorsBinding {
            executable: self.dir.join("connectors"),
            adapter: "gpu".to_owned(),
            connection: "conn-1".to_owned(),
            work_directory: self.dir.join("work"),
            timeout: Duration::from_secs(20),
        })
    }
}

/// Invariant 2: a `billing.pods` result that does not say it succeeded (no `status`) is a
/// partial answer, and a partial answer is never read as spend. Today `body()` takes a result
/// without a `status` as a success, so its empty `body` reads as zero spend.
#[test]
fn sec_a_billing_result_without_a_status_is_never_zero_spend() {
    let fixture = Fixture::new("billing_no_status");
    fixture.script(&json!({"operations invoke billing.pods": [{"stdout": {
        "adapter": "gpu", "operation": "billing.pods", "revision": "revision-7",
        "result": {"body": []},
    }}]}));
    let day_start = started_at_ms("2026-10-16T00:00:00.000Z").expect("day start");
    assert_eq!(
        fixture.transport().account_spend(day_start),
        None,
        "a result that carries no status was read as spend"
    );
}
