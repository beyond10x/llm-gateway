#![forbid(unsafe_code)]

//! The `b10x-llm-gateway` binary's deployment document, trusted-file reader and serving loop.
//!
//! The specification is `spec/domains/deployment.yaml` (`llm-gateway.deployment`). The binary
//! reads one closed TOML document through a same-handle trusted-file reader, composes the
//! `llm_gateway` gateway from it, serves, and stops gracefully on SIGINT or SIGTERM. Every
//! startup failure is one [`StartupRefusal`] with a stable `<source>:<rule>` code.

mod config;
mod keys;
mod logging;
mod refusal;
mod relaying;
mod serve;
mod trusted;

pub use config::{Deployment, Model, Provider, ProviderKind, Wire, load};
pub use keys::{VllmKey, VllmKeys, vllm_keys};
pub use logging::{TracingRecords, install_logging, log_filter};
pub use refusal::{Refusal, Source, StartupRefusal};
pub use relaying::{
    ACCOUNT, CLEANUP_INTERVAL, CONTROLLER, CRASH_RESTART_LIMIT, CRASH_WINDOW_MS, LEASE_MS, LEDGER,
    MAX_LIFETIME_MS, PROVIDER, PodConnector, ProxyConnector, RESERVATION, Relaying, WallClock,
    compute_authorization, hosting_policy, idle_timeout_ms, runpod_models, runpod_pool,
    secret_name, start_connected, start_relaying,
};
pub use serve::{Running, StopSignal, Stopped, inventory, owner_verifier, start};
