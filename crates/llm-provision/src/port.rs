//! The seam every hosting provider is held to.
//!
//! Three methods, and the split between them is the contract: exactly two of them can allocate
//! or destroy a billed resource, and the third one cannot. A provider adapter that provisions
//! inside `inventory` has broken the contract, not merely optimized it.

use crate::{DeploymentSpec, Dispatch, Identifier, Inventory, ObservedResource, ResourceKey};

/// One create mutation, with everything the provider needs to make it identifiable afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRequest {
    pub spec: DeploymentSpec,
    /// The idempotency key. Chosen and recorded by the controller *before* the mutation, so a
    /// resource created by a request whose answer was lost can still be found again.
    pub request_id: Identifier,
    /// The controller identity to tag the resource with.
    pub owner: Identifier,
    /// The fencing epoch to tag the resource with.
    pub epoch: u64,
}

/// What a create mutation told us — which is not the same as what it did.
///
/// `dispatch` is [`Dispatch::Unknown`] exactly when the answer was lost. `resource` is then
/// `None`, and that `None` means "we do not know", never "nothing was created".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOutcome {
    pub dispatch: Dispatch,
    pub resource: Option<ObservedResource>,
}

/// What a stop mutation told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopOutcome {
    pub dispatch: Dispatch,
    /// The provider's own reference for the stop. `None` unless the provider confirmed one.
    pub evidence: Option<Identifier>,
}

/// A hosting provider, as the ownership contract sees it.
pub trait HostingProvider {
    /// Submits a create mutation. May allocate a billed resource.
    fn create(&mut self, request: &CreateRequest) -> CreateOutcome;

    /// Submits a stop mutation for one exact resource identity.
    ///
    /// The `epoch` travels with the request so a provider that supports conditional writes can
    /// refuse a fenced controller itself. A provider that cannot is still fenced by the
    /// controller, which refuses before calling this.
    fn stop(&mut self, key: &ResourceKey, epoch: u64) -> StopOutcome;

    /// Reads the provider's own list of resources in this scope.
    ///
    /// **Allocates nothing.** Listing is how a restarted controller finds what it already owns;
    /// a listing that could create a resource would make reconciliation the thing that causes
    /// the double-billing it exists to prevent.
    fn inventory(&mut self) -> Inventory;

    /// Whether repeating [`CreateRequest::request_id`] is guaranteed to allocate at most once.
    ///
    /// A provider that answers `false` must never be retried after an unknown create: the only
    /// safe recovery is reconciliation against a complete inventory. The controller enforces
    /// this by refusing the retry with [`crate::HostingError::IdempotencyUnsupported`], rather than
    /// simulating a capability the provider does not document.
    fn honours_idempotency_key(&self) -> bool;
}
