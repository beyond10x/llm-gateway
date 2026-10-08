//! Adversarial pass over story:runpod-provider-description: what a pool hands out once the
//! provider refuses to build an address from a pod id.
//!
//! `RunpodPool::ensure` is documented as "Hands out a ready endpoint for a model"
//! (`src/pool.rs:410`). Before this change every running pod had an address, so a ready lease
//! always carried one. Now a running, serving pod whose id llm's description refuses reports no
//! endpoint, and the pool still calls it ready.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};

use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
use llm_runpod::{
    CloudType, CreateAnswer, EmulatedRunpod, ManualClock, PodListing, PodRequest, PoolError, Probe, ProbeTarget,
    RunpodModel, RunpodPool, RunpodTransport, TerminateAnswer, Thinking, VllmSettings,
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
    }
}

/// The emulator, with every pod id it issues rewritten to `pod-<n>`: an id the description
/// refuses. Everything else is the emulator's own behaviour.
#[derive(Debug, Clone)]
struct DashedIds(Arc<Mutex<EmulatedRunpod>>);

fn outbound(id: &str) -> String {
    id.strip_prefix("pod")
        .map_or_else(|| id.to_owned(), |n| format!("pod-{n}"))
}

fn inbound(id: &str) -> String {
    id.replacen("pod-", "pod", 1)
}

impl DashedIds {
    fn inner(&self) -> std::sync::MutexGuard<'_, EmulatedRunpod> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl RunpodTransport for DashedIds {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        let mut listing = self.inner().list_pods()?;
        for pod in &mut listing.pods {
            pod.id = outbound(&pod.id);
        }
        Ok(listing)
    }
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        match self.inner().create_pod(request) {
            CreateAnswer::Created(mut pod) => {
                pod.id = outbound(&pod.id);
                CreateAnswer::Created(pod)
            }
            other => other,
        }
    }
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        self.inner().terminate_pod(&inbound(pod_id))
    }
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let pod_id = inbound(target.pod_id);
        self.inner().probe_ready(&ProbeTarget {
            pod_id: &pod_id,
            ..*target
        })
    }
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        self.inner().container_started_at(&inbound(pod_id))
    }
}

/// A pool never hands out a "ready" lease that carries no address for a running pod: the
/// caller would hold a lease it cannot send a request to, and the pod it holds keeps billing.
#[test]
fn adv_w02_a_ready_lease_for_a_running_pod_with_a_refused_id_carries_an_address() {
    let transport = DashedIds(Arc::new(Mutex::new(EmulatedRunpod::new())));
    let clock = ManualClock::new(1_000);
    let pool = RunpodPool::new(
        policy(),
        Arc::new(LeaseRegistry::default()),
        transport.clone(),
        BTreeMap::from([(id(ALIAS), model())]),
        Arc::new(clock.clone()),
    )
    .expect("pool");
    let mut handed_out = Vec::new();
    for _ in 0..10 {
        match pool.ensure(&id(ALIAS), &authorization()) {
            Ok(lease) => handed_out.push((
                lease.deployment().as_str().to_owned(),
                lease.endpoint().map(str::to_owned),
            )),
            Err(PoolError::Starting) => {}
            Err(other) => panic!("unexpected refusal: {}", other.code()),
        }
        clock.advance(1_000);
    }
    let addressless: Vec<_> = handed_out
        .iter()
        .filter(|(_, endpoint)| endpoint.is_none())
        .collect();
    assert!(
        addressless.is_empty(),
        "ensure handed out {} ready leases with no endpoint for running pod(s) {:?}; \
         pods still running: {:?}",
        addressless.len(),
        addressless,
        transport
            .inner()
            .pods()
            .iter()
            .map(|pod| pod.id.clone())
            .collect::<Vec<_>>()
    );
}
