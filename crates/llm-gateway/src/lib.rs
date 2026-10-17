#![forbid(unsafe_code)]

//! Authenticated single-owner gateway composition.
//!
//! This crate admits exactly one authenticated owner and lets that owner read the routes a
//! deployment serves. It does four things and refuses the rest:
//!
//! - **Authenticate before anything else.** Apart from a closed two-path liveness and readiness
//!   surface and the two public routes (`GET /v1/models` and `GET /`, which render the relayed
//!   models and nothing else), no request is decoded past its HTTP head until an
//!   [`OwnerVerifier`] has accepted it, and every inspection entry point demands an
//!   [`Authenticated`] that only an accepted verdict can produce. The one exception is opt-in: a
//!   [`GatewayConfig`] whose [`IngressAuth`] is `None` relays model calls without a credential,
//!   and still authenticates everything else.
//! - **Inspect, read-only.** [`RouteInventory`] is an immutable snapshot with no field for an
//!   endpoint URL, a secret reference or credential material, and no callback, so serving it
//!   cannot provision anything. It renders exactly the identifier bytes the composer supplied
//!   and nothing else; a composer that puts a URL into an identifier field will see it, so the
//!   adapter that builds an inventory is responsible for passing identifiers.
//! - **Start and stop deliberately.** Liveness, readiness and a drain that keeps serving while
//!   reporting unready, followed by a graceful stop that reports what was in flight.
//! - **Relay three model-call wires.** Composed with a [`Relay`], the gateway relays the owner's
//!   `chat`, `responses` and `messages` requests to the target an injected [`RelayTargets`]
//!   hands out, rewriting only `model` (and on the messages wire a `high` or `max` effort, or
//!   adding a model's declared effort to a request naming none), and streams the answer back as
//!   it arrives. The target opens its own connection; this crate opens none.
//! - **Wake a model and read its phase.** Composed with a [`Relay`], the gateway serves the
//!   owner `POST /v1/models/{alias}/wake` and `GET /v1/models/{alias}/status`, answered from
//!   [`RelayTargets::wake`] and [`RelayTargets::status`] as a [`ModelStatus`]; a model whose
//!   source starts nothing for it has neither.
//! - **Hand other wires to a translator.** A request on a wire its model's upstream does not
//!   declare goes to the [`Translator`] the embedding composed with
//!   [`Relay::with_translator`], which decodes it before any target is asked for and writes
//!   the answer through a [`TranslatedAnswer`]; without one it is refused `wire-not-served`.
//!   The translator is the embedding's: this crate holds only the port.
//!
//! What it deliberately does not do: protocol translation itself (it holds the port, not a
//! protocol codec), resolving a secret, opening a connection, and multi-tenant accounts or
//! quotas. Credential material is
//! resolved by the embedding — from `llm-credentials` or anywhere else — and injected once, so
//! a server deployment never assumes a desktop keychain. See `docs/gateway.md`.

mod auth;
mod body;
mod error;
mod inventory;
mod json;
mod listing;
mod metrics;
mod relay;
mod server;
mod translate;

pub use auth::{
    Authenticated, NoOwnerVerifier, OwnerToken, OwnerVerifier, SharedSecretVerifier, Verdict,
};
pub use error::{
    BasePathError, CredentialHeaderError, InventoryError, LabelError, RefusalCode, RelayError,
    StaticHeaderError, TargetRefusal, TokenError, VerifierError,
};
pub use inventory::{
    AuthKind, BillingKind, Label, RouteInventory, RouteSummary, TargetLimits, TargetProvenance,
    TargetSummary,
};
pub use metrics::{Disposition, Metrics, UsageRecord, UsageRecords};
pub use relay::{
    BasePath, CredentialHeader, ModelPhase, ModelStatus, ReasoningEffort, Relay, RelayModel,
    RelayStream, RelayTarget, RelayTargets, StaticHeader, TargetBearer, TargetScheme, ToolCalling,
    Wire,
};
pub use server::{Gateway, GatewayConfig, GatewayHandle, IngressAuth, ShutdownReport};
pub use translate::{
    AnswerFraming, TranslatedAnswer, Translation, TranslationFailure, TranslationRequest,
    Translator,
};
