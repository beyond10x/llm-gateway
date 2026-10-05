#![forbid(unsafe_code)]

//! The `b10x-llm-gateway` binary's deployment document, trusted-file reader and serving loop.
//!
//! The specification is `spec/domains/deployment.yaml` (`llm-gateway.deployment`). The binary
//! reads one closed TOML document through a same-handle trusted-file reader, composes the
//! `llm_gateway` gateway from it, serves, and stops gracefully on SIGINT or SIGTERM. Every
//! startup failure is one [`StartupRefusal`] with a stable `<source>:<rule>` code.

mod config;
mod refusal;
mod serve;
mod trusted;

pub use config::{Deployment, Model, Provider, ProviderKind, Wire, load};
pub use refusal::{Refusal, Source, StartupRefusal};
pub use serve::{Running, StopSignal, Stopped, inventory, owner_verifier, start};
