#![forbid(unsafe_code)]

//! Runpod provisioning adapter for vLLM.
//!
//! Ported, with attribution, from llmgw `src/runpod.rs` (pod lifecycle), the Runpod part of
//! llmgw `src/config.rs` (the per-model settings) and the mock lifecycle tests in llmgw
//! `src/lib.rs`. The mechanics are llmgw's: ordered GPU fallback, one starter per cold model,
//! readiness polling with crash-loop detection (here by Runpod's `lastStartedAt` moving forward,
//! where llmgw read a decreasing uptime), an idle reaper whose limit never drops below the
//! measured cold start, in-flight leases that keep a streaming pod alive, and an orphan sweep.
//! What is new is that every one of them now runs under the `llm_provision` hosting contract:
//!
//! * [`RunpodProvider`] is a `llm_provision::HostingProvider`. A pod is a resource key whose
//!   incarnation is its Runpod pod id; owner, epoch and request id travel in the pod's
//!   environment ([`TAG_OWNER`], [`TAG_EPOCH`], [`TAG_REQUEST`]). Runpod takes no idempotency key,
//!   so a lost create is never retried — not even on the next GPU — and is resolved only by a
//!   listing that finds its request id.
//! * [`RunpodPool`] drives an `llm_provision::Controller`. Adoption after a restart is by the
//!   durable record's exact identity, never by name, so a pod that reused the name, or one
//!   labelled for another owner, is not taken over.
//! * Pod names start with [`POD_NAME_PREFIX`], which is not llmgw's [`LEGACY_POD_NAME_PREFIX`].
//!   Nothing here ever selects, adopts or terminates an llmgw pod.
//!
//! Runpod-specific settings — GPU choices, mounted caches, startup deadlines, vLLM arguments —
//! live in [`RunpodModel`], not in `DeploymentSpec`. Every Runpod call goes through
//! [`RunpodTransport`]; [`EmulatedRunpod`] is the in-process control plane the tests drive, and
//! [`ConnectorsRunpod`] the production transport, which reaches the control plane only through
//! the `connectors` CLI. This crate holds no control-plane credential and reads no credential
//! file: the vLLM key reaches a pod as a Runpod secret reference, and reaches the readiness probe
//! (a plain-HTTP `GET <endpoint>models`, the only connection the crate opens) as a value its
//! embedding hands in. It links `b10x-llm-credentials` and `tokio` through
//! `b10x-llm-providers` and uses neither: it calls only `descriptions::runpod` and
//! `inference_base_url`, which parse the shipped description and fill its URL template.

mod config;
mod emulated;
mod pool;
mod provider;
mod request;
mod transport;

pub use config::{CloudType, ConfigError, NetworkVolume, RunpodModel, Thinking, VllmSettings};
pub use emulated::EmulatedRunpod;
/// The hosting-contract values a [`RunpodPool`] is built from, so a composer can build one
/// without naming `b10x-llm-provision` itself.
pub use llm_provision::{ComputeAuthorization, HostingPolicy, Identifier, LeaseRegistry};
pub use pool::{CleanupReport, Clock, Hold, ManualClock, PoolError, RunpodPool, StreamLease};
pub use provider::{RunpodProvider, Unserviceable};
pub use request::{
    LEGACY_POD_NAME_PREFIX, NON_THINKING_SAMPLING, POD_NAME_PREFIX, TAG_EPOCH, TAG_OWNER,
    TAG_REQUEST, Tags, in_namespace, pod_name, pod_request, vllm_entrypoint,
};
pub use transport::{
    ConnectorsBinding, ConnectorsRunpod, CreateAnswer, POD_CREATE_BODY_KEYS, Pod, PodListing,
    PodRequest, PodStatus, Probe, ProbeTarget, RunpodTransport, TerminateAnswer, started_at_ms,
};
