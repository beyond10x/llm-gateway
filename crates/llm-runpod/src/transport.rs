//! The Runpod control-plane seam.
//!
//! Every call a Runpod adapter makes goes through [`RunpodTransport`], so the lifecycle can be
//! driven end to end by [`EmulatedRunpod`](crate::EmulatedRunpod) without a socket, an HTTP
//! client or a credential. Two production transports reach Runpod's control plane:
//!
//! * [`ConnectorsRunpod`] only through the `connectors` CLI and its Runpod catalog bundle
//!   (`spec/domains/runpod.yaml`, `ConnectorsOperation`). It never holds the Runpod key, so it
//!   is the transport for a gateway in a cluster.
//! * [`RestRunpod`] calls Runpod's REST API (`https://rest.runpod.io/v1`) itself with an API
//!   key its embedding hands in, for a local gateway whose process reads the key through an OS
//!   keychain reference. It speaks HTTP/1.1 over the [`PodTls`] connection the embedding hands
//!   in, and also implements [`RunpodAccount`], the account view a watchdog reads.
//!
//! Both probe a pod's own `GET <endpoint>models`: an `http://` endpoint over a plain socket, an
//! `https://` one over the TLS connection its embedding hands in ([`PodTls`]). The crate links
//! no HTTP client library and no TLS client.

mod connectors;
mod probe;
mod rest;

use std::collections::BTreeMap;

use crate::{AccountSpend, CloudType};

pub use connectors::{ConnectorsBinding, ConnectorsRunpod, POD_CREATE_BODY_KEYS, started_at_ms};
pub use probe::{PodStream, PodTls};
pub use rest::{RestBinding, RestRunpod};

/// Runpod's own word for a pod's desired state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodStatus {
    Created,
    Running,
    Exited,
    Terminated,
}

/// One pod as the control plane listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pod {
    /// Provider-assigned and never reused: the incarnation of a resource key.
    pub id: String,
    /// Chosen by whoever created the pod. **Not** an identity.
    pub name: String,
    pub status: PodStatus,
    /// The environment the pod was created with, which is where this adapter's owner, epoch
    /// and request tags live.
    pub env: BTreeMap<String, String>,
    pub gpu_type: Option<String>,
}

/// The result of one listing call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodListing {
    pub pods: Vec<Pod>,
    /// `false` for a truncated page or a throttled call. Absence then means nothing.
    pub complete: bool,
}

/// A create request, in the shape of Runpod's `POST /v1/pods` body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodRequest {
    pub name: String,
    pub image: String,
    pub gpu_type: String,
    pub gpu_count: u32,
    pub cloud_type: CloudType,
    pub container_disk_gb: u32,
    pub ports: Vec<String>,
    pub interruptible: bool,
    pub env: BTreeMap<String, String>,
    pub docker_entrypoint: Vec<String>,
    pub network_volume_id: Option<String>,
    pub volume_mount_path: Option<String>,
    pub data_center_ids: Vec<String>,
}

/// Where Runpod placed a pod: its machine's `secureCloud` and the pod's `interruptible`, as
/// `GET /pods/<id>?includeMachine=true` answers them. `None` is a field Runpod did not report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodPlacement {
    pub secure_cloud: Option<bool>,
    pub interruptible: Option<bool>,
}

impl PodPlacement {
    /// A Secure Cloud machine, and not an interruptible (spot) pod. Runpod omits
    /// `interruptible` for an on-demand pod (observed 2026-11-04), so an absent one counts as
    /// false; an absent `secureCloud` does not count as true.
    pub fn is_secure_on_demand(&self) -> bool {
        self.secure_cloud == Some(true) && self.interruptible != Some(true)
    }
}

/// One pod as the account view lists it, for an operator's watchdog or dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct PodSummary {
    pub id: String,
    pub name: String,
    /// Runpod's `desiredStatus` as it answered it (`RUNNING`, `EXITED`, …).
    pub desired_status: String,
    pub cost_per_hour_usd: Option<f64>,
    pub gpu: Option<String>,
    pub uptime_seconds: Option<u64>,
    /// Runpod's `lastStartedAt`, in milliseconds since the Unix epoch.
    pub last_started_at_ms: Option<u64>,
    /// The pod's [`TAG_OWNER`](crate::TAG_OWNER) environment value, if it carries one.
    pub env_owner: Option<String>,
}

/// The account as one read sees it: balance, spend rate and every pod.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountSnapshot {
    pub balance_usd: Option<f64>,
    pub spend_per_hour_usd: Option<f64>,
    pub pods: Vec<PodSummary>,
}

/// An account-wide read for an operator's watchdog or dashboard. Allocates nothing.
pub trait RunpodAccount {
    /// The account's balance, spend rate and pods; `None` when the read failed.
    fn account(&mut self) -> Option<AccountSnapshot>;
}

/// What a create call told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateAnswer {
    /// The control plane answered with the pod it created.
    Created(Pod),
    /// Refused for capacity: the provider has no machine of this GPU type free here (the
    /// `connectors` failure code `capacity`). Nothing was created.
    NoCapacity,
    /// Refused for any other reason. Nothing was created.
    Refused,
    /// The answer never arrived. A pod may or may not exist.
    Lost,
}

/// What a terminate call told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminateAnswer {
    Terminated,
    Refused,
    Lost,
}

/// What the pod's own `GET /v1/models` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// vLLM is serving, and listed these served model names.
    Ready { served_models: Vec<String> },
    /// The pod answered, and is not serving yet.
    NotReady,
    /// The pod refused the vLLM key (401/403): it holds a credential this deployment cannot use.
    Refused,
    /// No answer at all. Readiness is unknown, not `false`.
    Unreachable,
}

/// What a readiness probe is aimed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeTarget<'a> {
    /// The pod's provider id.
    pub pod_id: &'a str,
    /// The declared model alias the pod serves, which selects its vLLM key; `None` for a pod of
    /// no declared model.
    pub model: Option<&'a str>,
    /// The pod's inference base URL from llm's Runpod description, `/v1/` included; `None`
    /// when the pod has none.
    pub endpoint: Option<&'a str>,
}

/// The Runpod control plane and the pod probes, as calls.
pub trait RunpodTransport {
    /// Lists pods in the account. Allocates nothing.
    ///
    /// # Errors
    ///
    /// `Err(())` when the listing failed outright; the caller treats that as a partial listing.
    #[allow(clippy::result_unit_err)]
    fn list_pods(&mut self) -> Result<PodListing, ()>;
    /// Submits one create. May allocate a billed pod.
    fn create_pod(&mut self, request: &PodRequest) -> CreateAnswer;
    /// Terminates one pod by its provider id.
    fn terminate_pod(&mut self, pod_id: &str) -> TerminateAnswer;
    /// Probes the pod's vLLM server. Allocates nothing.
    fn probe_ready(&mut self, target: &ProbeTarget<'_>) -> Probe;
    /// When the pod's container last started, in milliseconds since the Unix epoch (Runpod's
    /// `lastStartedAt`), or `None` while that is unknown. A value later than the previous one
    /// means the container restarted.
    fn container_started_at(&mut self, pod_id: &str) -> Option<u64>;
    /// Where the pod was placed, read right after its create; `None` when the read failed or
    /// did not find exactly this pod. Allocates nothing. The provider terminates a pod at once
    /// unless this answers a [`PodPlacement::is_secure_on_demand`] placement.
    fn pod_placement(&mut self, pod_id: &str) -> Option<PodPlacement>;
    /// The account's reported spend: since `day_start_ms` (the current UTC day's start, in
    /// milliseconds since the Unix epoch) and in all. Allocates nothing. `None` when the read
    /// failed or answered nothing, which the spend check never takes for a met ceiling; a
    /// transport that cannot read spend keeps this default.
    fn account_spend(&mut self, day_start_ms: u64) -> Option<AccountSpend> {
        let _ = day_start_ms;
        None
    }
}
