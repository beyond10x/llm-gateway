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
    AccountSnapshot, AccountSpend, CreateAnswer, Pod, PodListing, PodPlacement, PodRequest,
    PodStatus, PodSummary, Probe, ProbeTarget, RunpodAccount, RunpodTransport, TerminateAnswer,
};

/// Where an emulated pod is placed unless a test says otherwise: a Secure Cloud machine, and
/// `interruptible` absent, as Runpod reports an on-demand pod.
const SECURE_ON_DEMAND: PodPlacement = PodPlacement {
    secure_cloud: Some(true),
    interruptible: None,
};

/// The instant every emulated container start is counted back from: a pod scripted with uptime
/// `u` reports [`RunpodTransport::container_started_at`] as this minus `u`.
const EMULATED_START_ORIGIN_MS: u64 = 1 << 52;

#[derive(Debug)]
struct Emulated {
    pod: Pod,
    served_name: Option<String>,
    probes_until_ready: u32,
    uptimes: VecDeque<u64>,
    credential_refused: bool,
    /// What a placement read answers; `None` is a read that fails.
    placement: Option<PodPlacement>,
    cost_per_hour_usd: Option<f64>,
    started_at_ms: Option<u64>,
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
    no_capacity_gpus: BTreeSet<String>,
    lose_next: Option<bool>,
    ready_after: u32,
    uptimes: Vec<u64>,
    partial: bool,
    unreachable: bool,
    /// What a spend read answers; `None`, the default, is a read that answers nothing.
    account_spend: Option<SpendScript>,
    spend_reads: u32,
    /// The placement the next created pod gets, once; `None` here is the default placement.
    next_placement: Option<Placed>,
    placement_reads: u32,
    balance_usd: Option<f64>,
    spend_per_hour_usd: Option<f64>,
    account_unanswered: bool,
}

/// The placement one created pod is scripted to get; `Placed(None)` is an unreadable one.
#[derive(Debug, Clone, Copy)]
struct Placed(Option<PodPlacement>);

/// What the emulated account's spend reads answer.
#[derive(Debug, Clone)]
enum SpendScript {
    /// The same spend whatever day start is asked: the script states today's spend.
    Stated(AccountSpend),
    /// Billed amounts in micro-USD at instants in ms: today's spend is those at or after the
    /// asked day start, the total all of them.
    Billed(Vec<(u64, u64)>),
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
    /// Spend reads made, whatever they answered.
    pub fn spend_reads(&self) -> u32 {
        self.state().spend_reads
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

    /// Refuses every create on this GPU type for a reason other than capacity
    /// ([`CreateAnswer::Refused`]).
    pub fn refuse_gpu(&self, gpu: &str) {
        self.state().refused_gpus.insert(gpu.to_owned());
    }
    /// Refuses every create on this GPU type for capacity ([`CreateAnswer::NoCapacity`]), as a
    /// region with no such machine free does.
    pub fn no_capacity_gpu(&self, gpu: &str) {
        self.state().no_capacity_gpus.insert(gpu.to_owned());
    }
    /// Loses the answer to the next create. `allocates` says whether the pod was made anyway.
    pub fn lose_next_create(&self, allocates: bool) {
        self.state().lose_next = Some(allocates);
    }
    /// Pods created from now on answer this many probes with "not ready" first.
    pub fn ready_after(&self, probes: u32) {
        self.state().ready_after = probes;
    }
    /// Pods created from now on report these container uptimes, one per poll. The emulator's
    /// clock stands still between polls, so a container up for `u` started at
    /// a fixed origin minus `u`: a falling uptime is a start time moving forward, which
    /// is how Runpod's `lastStartedAt` reports a restart.
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
    /// Spend reads from now on answer this; `None` answers nothing, as a failed read does.
    /// Until this is called every read answers nothing.
    pub fn set_account_spend(&self, spend: Option<AccountSpend>) {
        self.state().account_spend = spend.map(SpendScript::Stated);
    }
    /// Spend reads from now on answer from these billed amounts, each `(instant in ms,
    /// micro-USD)`: today's spend is the sum of those at or after the day start the read asks
    /// for, the total the sum of all of them, as Runpod's billing buckets are read.
    pub fn set_billed(&self, entries: Vec<(u64, u64)>) {
        self.state().account_spend = Some(SpendScript::Billed(entries));
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
            placement: Some(SECURE_ON_DEMAND),
            cost_per_hour_usd: None,
            started_at_ms: None,
        });
        id
    }

    /// The next pod created gets this placement instead of the default Secure Cloud, on-demand
    /// one; `None` makes its placement unreadable. Applies to one created pod.
    pub fn place_next_create(&self, placement: Option<PodPlacement>) {
        self.state().next_placement = Some(Placed(placement));
    }
    /// Sets what a placement read of an existing pod answers; `None` is a read that fails.
    pub fn set_placement(&self, pod_id: &str, placement: Option<PodPlacement>) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.placement = placement;
            }
        }
    }
    /// Placement reads made, whatever they answered.
    pub fn placement_reads(&self) -> u32 {
        self.state().placement_reads
    }
    /// What account reads answer from now on: the balance and the spend rate in USD. Until
    /// this is called both are unreported.
    pub fn set_account(&self, balance_usd: Option<f64>, spend_per_hour_usd: Option<f64>) {
        let mut state = self.state();
        state.balance_usd = balance_usd;
        state.spend_per_hour_usd = spend_per_hour_usd;
    }
    /// Account reads from now on fail (`true`) or answer (`false`, the default).
    pub fn account_unanswered(&self, unanswered: bool) {
        self.state().account_unanswered = unanswered;
    }
    /// Sets a pod's price per hour in USD, as an account read reports it.
    pub fn set_cost_per_hour(&self, pod_id: &str, usd: Option<f64>) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.cost_per_hour_usd = usd;
            }
        }
    }
    /// Sets when a pod's container last started, in milliseconds since the Unix epoch, as an
    /// account read reports it (Runpod's `lastStartedAt`).
    pub fn set_started_at(&self, pod_id: &str, at_ms: Option<u64>) {
        for pod in &mut self.state().pods {
            if pod.pod.id == pod_id {
                pod.started_at_ms = at_ms;
            }
        }
    }
}

/// Runpod's `desiredStatus` word for an emulated pod's status.
fn desired_status(status: PodStatus) -> &'static str {
    match status {
        PodStatus::Created => "CREATED",
        PodStatus::Running => "RUNNING",
        PodStatus::Exited => "EXITED",
        PodStatus::Terminated => "TERMINATED",
    }
}

impl RunpodAccount for EmulatedRunpod {
    /// The scripted balance and spend rate, and every existing pod with its scripted price and
    /// start time. Uptime is not emulated.
    fn account(&mut self) -> Option<AccountSnapshot> {
        let state = self.state();
        if state.account_unanswered {
            return None;
        }
        Some(AccountSnapshot {
            balance_usd: state.balance_usd,
            spend_per_hour_usd: state.spend_per_hour_usd,
            pods: state
                .pods
                .iter()
                .map(|pod| PodSummary {
                    id: pod.pod.id.clone(),
                    name: pod.pod.name.clone(),
                    desired_status: desired_status(pod.pod.status).to_owned(),
                    cost_per_hour_usd: pod.cost_per_hour_usd,
                    gpu: pod.pod.gpu_type.clone(),
                    uptime_seconds: None,
                    last_started_at_ms: pod.started_at_ms,
                    env_owner: pod.pod.env.get(crate::TAG_OWNER).cloned(),
                })
                .collect(),
        })
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
        if state.no_capacity_gpus.contains(&request.gpu_type) {
            return CreateAnswer::NoCapacity;
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
            placement: state
                .next_placement
                .take()
                .map_or(Some(SECURE_ON_DEMAND), |Placed(placement)| placement),
            cost_per_hour_usd: None,
            started_at_ms: None,
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

    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe {
        let pod_id = target.pod_id;
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

    fn container_started_at(&mut self, pod_id: &str) -> Option<u64> {
        let mut state = self.state();
        state
            .pods
            .iter_mut()
            .find(|pod| pod.pod.id == pod_id)
            .and_then(|pod| pod.uptimes.pop_front())
            .map(|uptime| EMULATED_START_ORIGIN_MS.saturating_sub(uptime))
    }

    fn pod_placement(&mut self, pod_id: &str) -> Option<PodPlacement> {
        let mut state = self.state();
        state.placement_reads = state.placement_reads.saturating_add(1);
        state
            .pods
            .iter()
            .find(|pod| pod.pod.id == pod_id)
            .and_then(|pod| pod.placement)
    }

    /// Answers the scripted spend: a stated spend whatever the day's start, billed amounts
    /// counted as today's from `day_start_ms` on.
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        let mut state = self.state();
        state.spend_reads = state.spend_reads.saturating_add(1);
        match state.account_spend.as_ref()? {
            SpendScript::Stated(spend) => Some(*spend),
            SpendScript::Billed(entries) => {
                let mut spend = AccountSpend {
                    daily_micro_usd: 0,
                    total_micro_usd: 0,
                };
                for &(at_ms, micro_usd) in entries {
                    spend.total_micro_usd = spend.total_micro_usd.checked_add(micro_usd)?;
                    if at_ms >= day_start_ms {
                        spend.daily_micro_usd = spend.daily_micro_usd.checked_add(micro_usd)?;
                    }
                }
                Some(spend)
            }
        }
    }
}
