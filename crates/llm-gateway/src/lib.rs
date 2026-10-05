#![forbid(unsafe_code)]

//! Authenticated single-owner gateway composition.
//!
//! This crate admits exactly one authenticated owner and lets that owner read the routes a
//! deployment serves. It does three things and refuses the rest:
//!
//! - **Authenticate before anything else.** Apart from a closed two-path liveness and readiness
//!   surface, no request is decoded past its HTTP head until an [`OwnerVerifier`] has accepted
//!   it, and every inspection entry point demands an [`Authenticated`] that only an accepted
//!   verdict can produce.
//! - **Inspect, read-only.** [`RouteInventory`] is an immutable snapshot with no field for an
//!   endpoint URL, a secret reference or credential material, and no callback, so serving it
//!   cannot provision anything. It renders exactly the identifier bytes the composer supplied
//!   and nothing else; a composer that puts a URL into an identifier field will see it, so the
//!   adapter that builds an inventory is responsible for passing identifiers.
//! - **Start and stop deliberately.** Liveness, readiness and a drain that keeps serving while
//!   reporting unready, followed by a graceful stop that reports what was in flight.
//!
//! What it deliberately does not do: protocol translation, proxying a model call, resolving a
//! secret, reaching a network, and multi-tenant accounts or quotas. Credential material is
//! resolved by the embedding — from `llm-credentials` or anywhere else — and injected once, so
//! a server deployment never assumes a desktop keychain. See `docs/gateway.md`.

mod auth;
mod error;
mod inventory;
mod json;
mod server;

pub use auth::{Authenticated, OwnerToken, OwnerVerifier, SharedSecretVerifier, Verdict};
pub use error::{InventoryError, LabelError, RefusalCode, TokenError, VerifierError};
pub use inventory::{
    AuthKind, BillingKind, Label, RouteInventory, RouteSummary, TargetLimits, TargetProvenance,
    TargetSummary,
};
pub use server::{Gateway, GatewayConfig, GatewayHandle, ShutdownReport};
