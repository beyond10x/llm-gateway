//! Stub: the production transport over the `connectors` CLI.

use std::{collections::BTreeMap, fmt, path::PathBuf, time::Duration};

use crate::{
    CreateAnswer, PodListing, PodRequest, Probe, ProbeTarget, RunpodTransport, TerminateAnswer,
};

/// The keys a `pod.create` body may carry.
pub const POD_CREATE_BODY_KEYS: &[&str] = &[];

/// What the transport is built from (`llm-gateway.runpod.ConnectorsBinding`).
#[derive(Debug, Clone)]
pub struct ConnectorsBinding {
    pub executable: PathBuf,
    pub adapter: String,
    pub connection: String,
    pub work_directory: PathBuf,
    pub timeout: Duration,
}

/// The production Runpod transport.
pub struct ConnectorsRunpod {
    binding: ConnectorsBinding,
    keys: BTreeMap<String, Vec<u8>>,
}

impl fmt::Debug for ConnectorsRunpod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectorsRunpod")
            .field("binding", &self.binding)
            .field("keys", &self.keys)
            .finish()
    }
}

impl ConnectorsRunpod {
    pub fn new(binding: ConnectorsBinding) -> Self {
        Self {
            binding,
            keys: BTreeMap::new(),
        }
    }

    pub fn set_vllm_key(&mut self, alias: &str, key: Vec<u8>) {
        self.keys.insert(alias.to_owned(), key);
    }
}

/// Runpod's `lastStartedAt` in milliseconds since the Unix epoch.
pub fn started_at_ms(_stamp: &str) -> Option<u64> {
    None
}

impl RunpodTransport for ConnectorsRunpod {
    fn list_pods(&mut self) -> Result<PodListing, ()> {
        Err(())
    }
    fn create_pod(&mut self, _request: &PodRequest) -> CreateAnswer {
        CreateAnswer::Refused
    }
    fn terminate_pod(&mut self, _pod_id: &str) -> TerminateAnswer {
        TerminateAnswer::Terminated
    }
    fn probe_ready(&mut self, _target: &ProbeTarget<'_>) -> Probe {
        Probe::NotReady
    }
    fn container_started_at(&mut self, _pod_id: &str) -> Option<u64> {
        None
    }
}
