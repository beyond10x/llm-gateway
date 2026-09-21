//! What the **provider** reported. Nothing in this module is ever filled in from a request.

use crate::{Identifier, ResourceKey, closed::closed_enum};

closed_enum! {
    /// What became of a submitted mutation.
    ///
    /// The four words are `llm_core::Dispatch`'s and `llm.catalog.Dispatch`'s, and they mean
    /// the same here: `Unknown` is the case the contract exists for. A create whose outcome is
    /// unknown may have allocated a billed resource, so it is never retried blindly and never
    /// treated as a no-op.
    Dispatch {
        /// The request never left this process. Nothing was allocated.
        NotSent => "not-sent", "the mutation was never sent";
        /// The provider refused. Nothing was allocated.
        Rejected => "rejected", "the provider refused the mutation";
        /// The outcome was never learned. A resource may or may not exist.
        Unknown => "unknown", "the mutation outcome is unknown";
        /// The provider answered with the resource it created.
        Accepted => "accepted", "the provider accepted the mutation";
    }
}

closed_enum! {
    /// The provider's own word for a resource's runtime state.
    ///
    /// Deliberately not the same thing as readiness. A `Running` resource whose readiness the
    /// provider does not report is not ready; it is unknown.
    ProviderState {
        Pending => "pending", "pending";
        Running => "running", "running";
        Terminated => "terminated", "terminated";
    }
}

closed_enum! {
    /// Whether a provider guarantees that repeating a request id allocates at most once.
    Idempotency {
        Honoured => "honoured", "repeating a request id allocates at most once";
        Unsupported => "unsupported", "repeating a request id may allocate again";
    }
}

closed_enum! {
    /// Whether an inventory listed every resource in the scope.
    ///
    /// `Partial` is the honest answer to a truncated page, a throttled list call or a
    /// region-scoped read. Absence from a `Partial` inventory carries no information at all.
    Completeness {
        Complete => "complete", "every resource in the scope was listed";
        Partial => "partial", "the listing is incomplete";
    }
}

/// One resource exactly as the provider described it.
///
/// Every optional field is `None` when the provider did not report it. None of them ever falls
/// back to the corresponding field of the [`DeploymentSpec`](crate::DeploymentSpec): an
/// unreported endpoint is an unknown endpoint, not the one that was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedResource {
    pub key: ResourceKey,
    pub state: ProviderState,
    /// `None` when the provider does not report readiness. Never coerced to `false`.
    pub ready: Option<bool>,
    /// `None` when the provider does not report an address.
    pub endpoint: Option<String>,
    /// `None` when the provider does not report which model is served.
    pub served_model: Option<Identifier>,
    /// The owner tag written at creation time, when the provider carries one back.
    pub owner: Option<Identifier>,
    /// The ownership epoch tag, when the provider carries one back.
    pub epoch: Option<u64>,
    /// The idempotency key the resource was created with, when the provider carries one back.
    pub request_id: Option<Identifier>,
}

/// The result of one listing call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    pub completeness: Completeness,
    pub resources: Vec<ObservedResource>,
}

impl Inventory {
    /// Whether this listing may be used to conclude that a resource is absent.
    pub fn complete(&self) -> bool {
        self.completeness == Completeness::Complete
    }

    /// The resource whose key matches exactly, including its incarnation.
    pub fn exact(&self, key: &ResourceKey) -> Option<&ObservedResource> {
        self.resources.iter().find(|resource| &resource.key == key)
    }

    /// Whether this listing can answer "was anything created under this request id?".
    ///
    /// Completeness alone cannot. It promises that every resource in the scope was listed; it
    /// promises nothing about whether each listed resource echoes the idempotency key it was
    /// created with, and [`ObservedResource::request_id`] is optional precisely because
    /// providers may not. A listing holding a resource with no request id cannot rule mine
    /// out, so concluding absence from it would turn an unresolved create into a false stop.
    pub fn answers_request_identity(&self) -> bool {
        self.complete()
            && self
                .resources
                .iter()
                .all(|resource| resource.request_id.is_some())
    }

    /// The resource created under this idempotency key, if the provider reports one.
    pub fn by_request(&self, request_id: &Identifier) -> Option<&ObservedResource> {
        self.resources
            .iter()
            .find(|resource| resource.request_id.as_ref() == Some(request_id))
    }
}

/// What is actually deployed, as last observed.
///
/// This is the counterpart of [`DeploymentSpec`](crate::DeploymentSpec) and shares no field
/// with it by construction: it can only be built from an [`ObservedResource`]. A caller that
/// needs the served model reads [`Self::served_model`] and gets `None` when nobody said; it
/// does not get the model that was requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedDeployment {
    /// The identity this record owns, including the provider-assigned incarnation.
    pub key: ResourceKey,
    /// `None` after a restart: a state observed before the restart is stale, not current.
    pub state: Option<ProviderState>,
    pub ready: Option<bool>,
    pub endpoint: Option<String>,
    pub served_model: Option<Identifier>,
    /// When the liveness facts above were observed.
    pub observed_at_ms: u64,
}

impl ProvisionedDeployment {
    /// Builds the observed record from a provider answer, and from nothing else.
    pub fn from_observation(resource: &ObservedResource, now_ms: u64) -> Self {
        Self {
            key: resource.key.clone(),
            state: Some(resource.state),
            ready: resource.ready,
            endpoint: resource.endpoint.clone(),
            served_model: resource.served_model.clone(),
            observed_at_ms: now_ms,
        }
    }

    /// Keeps the identity and drops every liveness fact.
    ///
    /// Used when a controller restarts. The resource key survives a restart because it is what
    /// the resource *is*; readiness, address, served model and runtime state do not, because
    /// they are what it *was doing* at a moment that has passed.
    #[must_use]
    pub fn forgetting_liveness(&self) -> Self {
        Self {
            key: self.key.clone(),
            state: None,
            ready: None,
            endpoint: None,
            served_model: None,
            observed_at_ms: self.observed_at_ms,
        }
    }

    /// Whether the resource was observed ready. Unknown readiness is not readiness.
    pub fn is_ready(&self) -> bool {
        self.ready == Some(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{Completeness, Inventory, ObservedResource, ProviderState, ProvisionedDeployment};
    use crate::{Identifier, ResourceKey};

    fn id(value: &str) -> Identifier {
        Identifier::new(value).expect("identifier")
    }

    fn resource() -> ObservedResource {
        ObservedResource {
            key: ResourceKey {
                provider: id("fake"),
                account: id("account-1"),
                name: id("pool-slot"),
                incarnation: id("one"),
            },
            state: ProviderState::Running,
            ready: Some(true),
            endpoint: Some("http://fake/one".to_owned()),
            served_model: Some(id("served-model")),
            owner: Some(id("controller-a")),
            epoch: Some(1),
            request_id: Some(id("request-d1")),
        }
    }

    #[test]
    fn a_restart_keeps_identity_and_forgets_liveness() {
        let observed = ProvisionedDeployment::from_observation(&resource(), 7);
        assert!(observed.is_ready());
        let restored = observed.forgetting_liveness();
        assert_eq!(restored.key, observed.key);
        assert_eq!(restored.state, None);
        assert_eq!(restored.ready, None);
        assert_eq!(restored.endpoint, None);
        assert_eq!(restored.served_model, None);
        assert!(!restored.is_ready());
    }

    #[test]
    fn a_complete_listing_without_request_identifiers_cannot_rule_a_request_out() {
        let mut anonymous = resource();
        anonymous.request_id = None;
        let listing = Inventory {
            completeness: Completeness::Complete,
            resources: vec![anonymous],
        };
        assert!(listing.complete());
        assert!(
            !listing.answers_request_identity(),
            "a listed resource with no idempotency key cannot rule another request out"
        );
        let answering = Inventory {
            completeness: Completeness::Complete,
            resources: vec![resource()],
        };
        assert!(answering.answers_request_identity());
        let partial = Inventory {
            completeness: Completeness::Partial,
            resources: vec![resource()],
        };
        assert!(!partial.answers_request_identity());
        let empty = Inventory {
            completeness: Completeness::Complete,
            resources: Vec::new(),
        };
        assert!(
            empty.answers_request_identity(),
            "nothing listed rules everything out"
        );
    }

    #[test]
    fn a_partial_inventory_may_not_be_used_to_conclude_absence() {
        let partial = Inventory {
            completeness: Completeness::Partial,
            resources: Vec::new(),
        };
        assert!(!partial.complete());
        assert!(partial.exact(&resource().key).is_none());
    }

    #[test]
    fn an_exact_match_requires_the_incarnation_too() {
        let inventory = Inventory {
            completeness: Completeness::Complete,
            resources: vec![resource()],
        };
        let mut reused = resource().key;
        reused.incarnation = id("two");
        assert!(inventory.exact(&resource().key).is_some());
        assert!(inventory.exact(&reused).is_none());
        assert!(reused.same_name(&resource().key));
    }
}
