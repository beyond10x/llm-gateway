#![forbid(unsafe_code)]

//! Hosting lifecycle ports and resource ownership contracts.
//!
//! This crate is the lifecycle every hosting provider adapter is held to, plus
//! [`FakeProvider`], an in-process provider that demonstrates it. It opens no socket, reads no
//! credential and allocates no cloud resource; it has no dependencies at all.
//!
//! The contract is documented in `docs/hosting.md`. Three of its rules explain most of the
//! shape of this API:
//!
//! * **A name is not an identity.** A provider may hand a resource name to a later resource
//!   once the previous one is gone, so [`ResourceKey`] carries the provider-assigned
//!   `incarnation` and equality includes it. A controller that matched on name alone would
//!   adopt, bill against and eventually stop somebody else's resource.
//! * **Requested is not observed.** [`DeploymentSpec`] is what was asked for and
//!   [`ProvisionedDeployment`] is what a provider reported. No field of the second is ever
//!   filled in from the first: an unreported served model, address or readiness stays `None`.
//! * **Nothing but evidence discharges a stop obligation.** Disconnecting a client, releasing
//!   or expiring a lease, restarting the controller and reading a truncated listing all leave
//!   the obligation exactly where it was. The vocabulary is `llm-cost`'s — `Uncertain`,
//!   `StopRequired`, `Stopped`, `stop_required` — because it is the same obligation the budget
//!   ledger already models from the other side; see `docs/budgets.md`.

mod closed;
mod controller;
mod error;
mod fake;
mod identity;
mod lease;
mod machine;
mod observation;
mod port;
mod spec;

pub use controller::{
    Controller, DeploymentRecord, EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY,
    EVIDENCE_PROVIDER_TERMINATED, HostingCommand, HostingReceipt, HostingTotals, HostingView,
};
pub use error::HostingError;
pub use fake::{AmbiguousCreate, FakeProvider};
pub use identity::{Identifier, InvalidIdentifier, ResourceKey};
pub use lease::{Lease, LeaseRegistry};
pub use machine::{Phase, StopReason, TRANSITIONS};
pub use observation::Idempotency;
pub use observation::{
    Completeness, Dispatch, Inventory, ObservedResource, ProviderState, ProvisionedDeployment,
};
pub use port::{CreateOutcome, CreateRequest, HostingProvider, StopOutcome};
pub use spec::{ComputeAuthorization, DeploymentSpec, HostingPolicy, MAX_ACTIVE_CEILING};

#[cfg(test)]
mod tests {
    use super::{
        EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY, EVIDENCE_PROVIDER_TERMINATED, Identifier,
    };

    #[test]
    fn every_generated_identifier_this_crate_publishes_is_a_valid_identifier() {
        // `Identifier::generated` skips validation so that no public entry point can panic.
        // That is only sound while every value it is handed is one a validating constructor
        // would have accepted, which is what this checks for the published constants.
        for value in [
            EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY,
            EVIDENCE_PROVIDER_TERMINATED,
        ] {
            assert!(Identifier::new(value).is_ok(), "{value}");
        }
    }

    #[test]
    fn the_generated_fixture_identifiers_are_valid_for_every_reachable_counter() {
        // The fake's generated names are `<prefix>-<u64>`; check both ends of the range.
        for counter in [0_u64, 1, u64::MAX] {
            for prefix in ["fake-incarnation", "foreign-request", "fake-stop"] {
                assert!(Identifier::new(format!("{prefix}-{counter}")).is_ok());
            }
        }
        assert!(Identifier::new("fake-served-model").is_ok());
    }
}
