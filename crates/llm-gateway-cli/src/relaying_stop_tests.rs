//! The binary's target source once the gateway starts to stop (story:model-wake-and-status),
//! over the emulator. A test module of `relaying.rs`, kept in its own file so that the shipped
//! module's source names no emulator (`b8_the_shipped_binary_composes_no_emulator_and_no_relay_seam`
//! in tests/relaying.rs reads it).

use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

/// Reaches no pod: the wake under test never connects.
struct NoPods;

impl super::PodConnector for NoPods {
    fn connect(&self, _authority: &str) -> std::io::Result<Box<dyn llm_gateway::RelayStream>> {
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionRefused))
    }
}

/// story:model-wake-and-status: once the gateway starts to stop, a wake starts nothing. It
/// answers the status of a model already starting or ready, and `target-unavailable` for
/// one that is not, and makes no pool step for either.
#[test]
fn a_wake_once_the_gateway_stops_starts_nothing() {
    use llm_gateway::{Metrics, ModelPhase, RelayTargets, TargetBearer, TargetRefusal};
    use llm_runpod::{EmulatedRunpod, Identifier, ManualClock};
    use std::{collections::BTreeMap, sync::atomic::AtomicBool};
    let document = "listen = \"127.0.0.1:0\"\nowner_secret_file = \"/run/owner-secret\"\n\n[providers.runpod]\nkind = \"runpod-vllm\"\n\n[models.small]\nprovider = \"runpod\"\nwires = [\"chat\"]\ncontext_window = 65536\nhf_model = \"example/small-model\"\nimage = \"vllm/vllm-openai:v0.27.1\"\ngpu_types = [\"NVIDIA L40S\"]\nmax_model_len = 1024\n";
    let deployment = crate::parse(document).unwrap();
    let runpod = EmulatedRunpod::new();
    runpod.ready_after(5);
    let pool = Arc::new(
        super::runpod_pool(
            &deployment,
            runpod.clone(),
            Arc::new(ManualClock::new(1_000)),
        )
        .unwrap(),
    );
    let relayed = BTreeMap::from([(
        "small".to_owned(),
        super::Relayed {
            alias: Identifier::new("small").unwrap(),
            bearer: Arc::new(TargetBearer::new(b"key".to_vec()).unwrap()),
            hold: Duration::ZERO,
            windows: BTreeMap::new(),
        },
    )]);
    let stopping = Arc::new(AtomicBool::new(true));
    let metrics = Arc::new(Metrics::default());
    let targets = super::PoolTargets::from_relayed(
        relayed,
        super::compute_authorization().unwrap(),
        pool,
        Arc::new(NoPods),
        Arc::clone(&stopping),
        Arc::clone(&metrics),
    );
    let refused = targets.wake("small").expect("a pod model has a lifecycle");
    assert_eq!(refused.err(), Some(TargetRefusal::Unavailable));
    assert_eq!(runpod.create_calls(), 0);
    stopping.store(false, Ordering::SeqCst);
    let woken = targets.wake("small").unwrap().unwrap();
    assert_eq!(woken.phase, ModelPhase::Starting);
    assert!(metrics.render().contains("llmgw_pod_starts_total 1"));
    stopping.store(true, Ordering::SeqCst);
    let lists = runpod.lists();
    let during = targets.wake("small").unwrap().unwrap();
    assert_eq!(during.phase, ModelPhase::Starting);
    assert_eq!(
        runpod.lists(),
        lists,
        "a wake during the stop stepped the pool"
    );
    assert_eq!(runpod.create_calls(), 1);
    assert_eq!(targets.status("small").unwrap().phase, ModelPhase::Starting);
    assert!(targets.wake("absent").is_none());
    assert!(targets.status("absent").is_none());
}
