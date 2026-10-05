//! The Runpod control-plane seam.
//!
//! Every call a Runpod adapter makes goes through [`RunpodTransport`], so the lifecycle can be
//! driven end to end by [`EmulatedRunpod`](crate::EmulatedRunpod) without a socket, an HTTP
//! client or a credential. A production transport speaks Runpod's REST API
//! (`GET/POST /v1/pods`, `DELETE /v1/pods/{id}`), its GraphQL runtime query and the pod's own
//! `GET /v1/models`; none is written here, and nothing in this crate opens a connection.

use std::collections::BTreeMap;

use crate::CloudType;

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

/// What a create call told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateAnswer {
    /// The control plane answered with the pod it created.
    Created(Pod),
    /// Refused — for Runpod usually "no capacity for this GPU type here". Nothing was created.
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
    fn probe_ready(&mut self, pod_id: &str) -> Probe;
    /// Container uptime in seconds, or `None` while the runtime is unavailable. A value lower
    /// than the previous one means the container restarted.
    fn container_uptime(&mut self, pod_id: &str) -> Option<u64>;
}
