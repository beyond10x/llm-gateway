#![forbid(unsafe_code)]

//! The `b10x-llm-gateway` binary's deployment document, trusted-file reader and serving loop.
//!
//! The specification is `spec/domains/deployment.yaml` (`llm-gateway.deployment`). The binary
//! reads one closed TOML document through a same-handle trusted-file reader, composes the
//! `llm_gateway` gateway from it, serves, and stops gracefully on SIGINT or SIGTERM. Every
//! startup failure is one [`StartupRefusal`] with a stable `<source>:<rule>` code.

pub mod cli;
mod config;
mod hosted;
mod keys;
mod logging;
mod refusal;
mod relaying;
mod serve;
mod tls;
mod trusted;

pub use config::{Deployment, Hosted, Model, Provider, ProviderKind, Wire, load, parse};
pub use hosted::{
    BaseUrlError, ENDPOINT_CONNECT_TIMEOUT, ENDPOINT_IO_TIMEOUT, EndpointCredential,
    HeaderSetError, HostedEndpoint, HostedTargets, MAX_STATIC_HEADERS, Scheme, check_headers,
};
pub use keys::{VllmKey, VllmKeys, vllm_keys};
pub use logging::{TracingRecords, install_logging, log_filter};
pub use refusal::{Refusal, Source, StartupRefusal};
pub use relaying::{
    ACCOUNT, CLEANUP_INTERVAL, CONTROLLER, CRASH_RESTART_LIMIT, CRASH_WINDOW_MS, LEASE_MS, LEDGER,
    MAX_LIFETIME_MS, PROVIDER, PodConnector, RESERVATION, Relaying, WallClock,
    compute_authorization, hosting_policy, idle_timeout_ms, runpod_models, runpod_pool,
    secret_name, start_connected, start_relaying,
};
pub use serve::{
    Running, StopSignal, Stopped, inventory, main_with, owner_verifier, start, start_with,
};
pub use tls::ProxyConnector;
