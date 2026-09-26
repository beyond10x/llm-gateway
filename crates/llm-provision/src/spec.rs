//! What was **requested**: the controller's policy, the deployment specification and the
//! budget authorization that has to exist before a billed resource may be created.

use crate::{Identifier, error::HostingError};

/// The immutable scope one hosting controller owns.
///
/// One controller serves exactly one provider, one account and one budget ledger. A different
/// provider, account or ledger is a different administrative scope, exactly as a different
/// directory or policy id is a different scope for `llm-cost`'s ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingPolicy {
    /// The controller's own identity. It is written onto every resource it creates.
    pub controller: Identifier,
    pub provider: Identifier,
    pub account: Identifier,
    /// The budget ledger every compute obligation in this scope is reserved against.
    pub ledger: Identifier,
    /// Resource ceiling: the largest number of outstanding owned resources.
    pub max_active: u32,
    /// Time ceiling: the longest lifetime any resource in this scope may be given.
    pub max_lifetime_ms: u64,
    /// How long one ownership lease is granted for.
    pub lease_ms: u64,
}

/// The largest resource ceiling a policy may declare.
pub const MAX_ACTIVE_CEILING: u32 = 4_096;

impl HostingPolicy {
    /// Checks the ceilings a controller cannot operate without.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::InvalidPolicy`] when the resource ceiling is zero or above
    /// [`MAX_ACTIVE_CEILING`], or when either the time ceiling or the lease duration is zero.
    /// A zero time ceiling would authorize an unbounded paid resource; there is no
    /// "unlimited" spelling on purpose.
    pub fn validate(&self) -> Result<(), HostingError> {
        if self.max_active == 0
            || self.max_active > MAX_ACTIVE_CEILING
            || self.max_lifetime_ms == 0
            || self.lease_ms == 0
        {
            return Err(HostingError::InvalidPolicy);
        }
        Ok(())
    }
}

/// What the operator asked for. Never mixed with what the provider reported.
///
/// Every field here is a request. The corresponding observed facts live in
/// [`ProvisionedDeployment`](crate::ProvisionedDeployment) and are populated only from a
/// provider answer. A reader that wants to know which model is actually being served must read
/// the observation, which may be absent; the requested `model` is not a substitute for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentSpec {
    /// The operator's own name for this deployment. Identity within the controller.
    pub id: Identifier,
    pub provider: Identifier,
    pub account: Identifier,
    /// The model the operator intends the resource to serve.
    pub model: Identifier,
    pub image: Identifier,
    /// The name to ask the provider for. Not an identity: a provider may reuse it later.
    pub resource_name: Identifier,
    /// How long the operator intends to keep the resource. Bounded by the policy's ceiling.
    pub requested_lifetime_ms: u64,
}

impl DeploymentSpec {
    /// Checks the specification against the scope it would run in. Allocates nothing.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::InvalidSpec`] when the specification names another provider or
    /// account, or asks for a lifetime above the policy's time ceiling.
    pub fn validate(&self, policy: &HostingPolicy) -> Result<(), HostingError> {
        if self.provider != policy.provider
            || self.account != policy.account
            || self.requested_lifetime_ms == 0
            || self.requested_lifetime_ms > policy.max_lifetime_ms
        {
            return Err(HostingError::InvalidSpec);
        }
        Ok(())
    }

    /// The effective lifetime: the requested one, never above the policy ceiling.
    pub fn effective_lifetime_ms(&self, policy: &HostingPolicy) -> u64 {
        self.requested_lifetime_ms.min(policy.max_lifetime_ms)
    }
}

/// The caller's proof that this compute operation is already admitted by a budget ledger.
///
/// The two fields name `llm-cost`'s own objects: `ledger` is that crate's `BudgetPolicy::id` and
/// `reservation` is its `ReservationRequest::id` for a `Operation::Compute` reservation. The
/// hosting controller cannot see the ledger and does not re-derive admission; it refuses to
/// create a billed resource that is not attached to one, and it reports stop obligations in
/// terms the ledger already understands (`RequireStop` / `ConfirmStopped`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeAuthorization {
    pub ledger: Identifier,
    pub reservation: Identifier,
}
