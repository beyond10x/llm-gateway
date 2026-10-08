//! An in-process Runpod control plane that allocates nothing and reaches nothing.
//!
//! The successor of the axum mock in llmgw `src/lib.rs` (`mock_router`, `mock_delete`,
//! `mock_graphql`, `mock_models`). That mock listened on a local socket; this one is a vector
//! behind a mutex. It can answer everything the lifecycle has to survive: a GPU with no capacity,
//! a create answer lost after the pod was made, a pod that never becomes ready, a crash-looping
//! container, a rotated credential, an out-of-band termination, a truncated listing, and pods
//! created by somebody else.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use crate::{
    CreateAnswer, Pod, PodListing, PodRequest, PodStatus, Probe, RunpodTransport, TerminateAnswer,
};

#[derive(Debug)]
struct Emulated {
    pod: Pod,
    served_name: Option<String>,
    probes_until_ready: u32,
    uptimes: VecDeque<u64>,
    credential_refused: bool,
}

#[derive(Debug, Default)]
struct State {
    pods: Vec<Emulated>,
    next_pod: u64,
    create_calls: u32,
    lists: u32,
    requests: Vec<PodRequest>,
    terminations: Vec<String>,
    refused_gpus: BTreeSet<String>,
    lose_next: Option<bool>,
    ready_after: u32,
    uptimes: Vec<u64>,
    partial: bool,
    unreachable: bool,
}

/// A scriptable, shareable Runpod control plane. Clones share one state.
#[derive(Debug, Clone, Default)]
pub struct EmulatedRunpod {
    state: Arc<Mutex<State>>,
}

impl EmulatedRunpod {
    /// A control plane with no pods, capacity on every GPU, and pods that serve on first probe.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `pod1`, `pod2`, …: ids llm's Runpod description accepts (`[a-z0-9]`, 1-48 bytes), so
    /// every running emulated pod has an endpoint.
    fn next_id(state: &mut State) -> String {
        state.next_pod = state.next_pod.saturating_add(1);
        format!("pod{}", state.next_pod)
    }

    /// Create calls submitted, whatever they answered.
    pub fn create_calls(&self) -> u32 {
        self.state().create_calls
    }
    /// Listing calls.
    pub fn lists(&self) -> u32 {
        self.state().lists
    }
    /// Every create request submitted, in order.
    pub fn requests(&self) -> Vec<PodRequest> {
        self.state().requests.clone()
    }
    /// Pod ids successfully terminated, in order.
    pub fn terminations(&self) -> Vec<String> {
        self.state().terminations.clone()
    }
    /// Pods that currently exist.
    pub fn pods(&self) -> Vec<Pod> {
        self.state()
            .pods
            .iter()
            .map(|pod| pod.pod.clone())
            .collect()
    }

    /// Refuses every create on this GPU type, as a region without capacity does.
    pub fn refuse_gpu(&self, gpu: &str) {
        self.state().refused_gpus.insert(gpu.to_owned());
    }
    /// Loses the answer to the next create. `allocates` says whether the pod was made anyway.
    pub fn lose_next_create(&self, allocates: bool) {
        self.state().lose_next = Some(allocates);
    }
    /// Pods created from now on answer this many probes with "not ready" first.
    pub fn ready_after(&self, probes: u32) {
        self.state().ready_after = probes;
    }
    /// Pods created from now on report these container uptimes, one per poll.
    pub fn uptimes(&self, script: Vec<u64>) {
        self.state().uptimes = script;
    }
    /// The pod now refuses the vLLM key, as one holding a rotated credential does.
    pub fn refuse_credential(&self, pod_id: &str) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.credential_refused = true;
            }
        }
    }
    /// Listings from now on claim to be incomplete.
    pub fn partial_listing(&self, partial: bool) {
        self.state().partial = partial;
    }
    /// Probes from now on get no answer at all.
    pub fn probe_unreachable(&self, unreachable: bool) {
        self.state().unreachable = unreachable;
    }
    /// Sets the status Runpod reports for a pod, as a stop (`Exited`) or a slow start (`Created`)
    /// would.
    pub fn set_status(&self, pod_id: &str, status: PodStatus) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.pod.status = status;
            }
        }
    }
    /// Removes a pod without anybody here asking, as a console termination or a host reclaim does.
    pub fn vanish(&self, pod_id: &str) {
        self.state().pods.retain(|pod| pod.pod.id != pod_id);
    }
    /// Rewrites a pod's owner and epoch tags, as a takeover by another controller would.
    pub fn retag(&self, pod_id: &str, owner: &str, epoch: u64) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.pod
                    .env
                    .insert(crate::TAG_OWNER.to_owned(), owner.to_owned());
                pod.pod
                    .env
                    .insert(crate::TAG_EPOCH.to_owned(), epoch.to_string());
            }
        }
    }
    /// Adds a running pod created by somebody else. Returns its id.
    pub fn insert_pod(&self, name: &str, env: BTreeMap<String, String>) -> String {
        let mut state = self.state();
        let id = Self::next_id(&mut state);
        state.pods.push(Emulated {
            pod: Pod {
                id: id.clone(),
                name: name.to_owned(),
                status: PodStatus::Running,
                env,
                gpu_type: None,
            },
            served_name: None,
            probes_until_ready: 0,
            uptimes: VecDeque::new(),
            credential_refused: false,
        });
        id
    }
}

fn served_name(request: &PodRequest) -> Option<String> {
    let args = &request.docker_entrypoint;
    args.iter()
        .position(|arg| arg == "--served-model-name")
        .and_then(|index| args.get(index + 1))
        .cloned()
}

impl RunpodTransport for EmulatedRunpod {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        let mut state = self.state();
        state.lists = state.lists.saturating_add(1);
        Ok(PodListing {
            pods: state.pods.iter().map(|pod| pod.pod.clone()).collect(),
            complete: !state.partial,
        })
    }

    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer {
        let mut state = self.state();
        state.create_calls = state.create_calls.saturating_add(1);
        state.requests.push(request.clone());
        if state.refused_gpus.contains(&request.gpu_type) {
            return CreateAnswer::Refused;
        }
        let lost = state.lose_next.take();
        if lost == Some(false) {
            return CreateAnswer::Lost;
        }
        let id = Self::next_id(&mut state);
        let pod = Pod {
            id,
            name: request.name.clone(),
            status: PodStatus::Running,
            env: request.env.clone(),
            gpu_type: Some(request.gpu_type.clone()),
        };
        let emulated = Emulated {
            pod: pod.clone(),
            served_name: served_name(request),
            probes_until_ready: state.ready_after,
            uptimes: state.uptimes.iter().copied().collect(),
            credential_refused: false,
        };
        state.pods.push(emulated);
        if lost == Some(true) {
            return CreateAnswer::Lost;
        }
        CreateAnswer::Created(pod)
    }

    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer {
        let mut state = self.state();
        let before = state.pods.len();
        state.pods.retain(|pod| pod.pod.id != pod_id);
        if state.pods.len() == before {
            return TerminateAnswer::Refused;
        }
        state.terminations.push(pod_id.to_owned());
        TerminateAnswer::Terminated
    }

    fn probe_ready(&mut self, pod_id: &str) -> Probe {
        let mut state = self.state();
        if state.unreachable {
            return Probe::Unreachable;
        }
        let Some(pod) = state.pods.iter_mut().find(|pod| pod.pod.id == pod_id) else {
            return Probe::Unreachable;
        };
        if pod.credential_refused {
            return Probe::Refused;
        }
        if pod.probes_until_ready > 0 {
            pod.probes_until_ready -= 1;
            return Probe::NotReady;
        }
        Probe::Ready {
            served_models: pod.served_name.iter().cloned().collect(),
        }
    }

    fn container_uptime(&mut self, pod_id: &str) -> Option<u64> {
        let mut state = self.state();
        state
            .pods
            .iter_mut()
            .find(|pod| pod.pod.id == pod_id)
            .and_then(|pod| pod.uptimes.pop_front())
    }
}
