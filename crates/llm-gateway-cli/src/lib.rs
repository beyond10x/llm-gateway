#![forbid(unsafe_code)]

//! The `b10x-llm-gateway` binary's deployment document, trusted-file reader, serving loop and
//! translator ([`LlmTranslator`], over llm 0.15.0's client crates and a tokio runtime).
//!
//! The specification is `spec/domains/deployment.yaml` (`llm-gateway.deployment`). The binary
//! reads one closed TOML document through a same-handle trusted-file reader, composes the
//! `llm_gateway` gateway from it, and from the `llm.routes/1` document it may name ([`Routes`]),
//! serves, and stops gracefully on SIGINT or SIGTERM. Every
//! startup failure is one [`StartupRefusal`] with a stable `<source>:<rule>` code.

mod bedrock;
pub mod cli;
mod config;
mod credential;
mod hosted;
mod keys;
mod logging;
mod refusal;
mod relaying;
mod routes;
mod seal;
mod serve;
mod tls;
mod translate;
mod trusted;

pub use config::{Bedrock, Deployment, Hosted, Model, Provider, ProviderKind, Wire, load, parse};
pub use credential::{
    CredentialFailure, CredentialTarget, MAX_KEY_BYTES, Probe, TargetKind, check_key,
};
pub use hosted::{
    BaseUrlError, ENDPOINT_CONNECT_TIMEOUT, ENDPOINT_IO_TIMEOUT, EndpointCredential,
    HeaderSetError, HostedEndpoint, HostedTargets, MAX_STATIC_HEADERS, Scheme, check_headers,
};
pub use keys::{VllmKey, VllmKeys, vllm_keys};
pub use logging::{TracingRecords, install_logging, log_filter};
pub use refusal::{CredentialRefusal, Refusal, Source, StartupRefusal};
pub use relaying::{
    ACCOUNT, CLEANUP_INTERVAL, CONTROLLER, CRASH_RESTART_LIMIT, CRASH_WINDOW_MS, LEASE_MS, LEDGER,
    MAX_LIFETIME_MS, METERED_RESERVATION, PROVIDER, PodConnector, RESERVATION, Relaying, WallClock,
    compute_authorization, compute_authorization_for, hosting_policy, idle_timeout_ms,
    runpod_models, runpod_pool, secret_name, spend_policy, start_connected, start_relaying,
};
pub use routes::{MAX_ROUTE_NAME_BYTES, Route, RouteEndpoint, Routes, SecretFile};
pub use serve::{
    INGRESS_AUTH_OFF, Running, StopSignal, Stopped, inventory, main_with, metrics_bearer,
    owner_verifier, start, start_with,
};
pub use tls::ProxyConnector;
pub use translate::{
    DEFAULT_CONTEXT_WINDOW, LlmTranslator, MESSAGES_DEFAULT_OUTPUT_TOKENS, NO_REASONING_EFFORT,
    REASONING_EFFORTS, TRANSLATION_CONNECT_TIMEOUT, TRANSLATION_READ_TIMEOUT,
    TRANSLATION_TURN_TIMEOUT,
};
