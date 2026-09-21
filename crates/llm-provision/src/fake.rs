//! An in-process hosting provider that allocates nothing and reaches nothing.
//!
//! This is the provider the contract is demonstrated against. It has no network client, no
//! credential and no cloud SDK; "allocating" here means appending a record to a vector. What it
//! does have is every answer a real control plane gives that the contract has to survive: a lost
//! create answer, a truncated listing, a name handed to somebody else's later resource, a
//! resource retagged by a newer owner, and a provider that will not promise idempotency.

use std::collections::BTreeSet;

use crate::{
    Completeness, CreateOutcome, CreateRequest, Dispatch, HostingProvider, Idempotency, Identifier,
    Inventory, ObservedResource, ProviderState, ResourceKey, StopOutcome, closed::closed_enum,
};

/// One optional thing a provider may or may not carry back about a resource.
///
/// Held as a set rather than a row of flags so that adding another optional field is not a
/// fifth boolean, and so every one of them defaults the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reported {
    Endpoint,
    ServedModel,
    OwnerTag,
    RequestId,
}

closed_enum! {
    /// What a create whose answer was lost actually did at the provider.
    ///
    /// The controller cannot see this and must behave correctly under either. It is a fixture
    /// knob, not a field of the contract.
    AmbiguousCreate {
        /// The resource exists; only the answer was lost.
        Allocates => "allocates", "the lost create did allocate";
        /// Nothing was created.
        DoesNotAllocate => "does-not-allocate", "the lost create allocated nothing";
    }
}

#[derive(Debug, Clone)]
struct Stored {
    key: ResourceKey,
    request_id: Identifier,
    owner: Identifier,
    epoch: u64,
    terminated: bool,
}

/// A scriptable hosting provider for tests and conformance observation.
#[derive(Debug)]
pub struct FakeProvider {
    provider: Identifier,
    account: Identifier,
    idempotency: Idempotency,
    completeness: Completeness,
    ambiguous: AmbiguousCreate,
    readiness: Option<bool>,
    reported: BTreeSet<Reported>,
    next_create: Option<Dispatch>,
    next_stop: Option<Dispatch>,
    resources: Vec<Stored>,
    incarnations: u64,
    allocations: u32,
    submits: u32,
    stop_calls: u32,
    lists: u32,
}

impl FakeProvider {
    /// A provider that accepts creates, reports everything and lists completely.
    pub fn new(provider: Identifier, account: Identifier) -> Self {
        Self {
            provider,
            account,
            idempotency: Idempotency::Honoured,
            completeness: Completeness::Complete,
            ambiguous: AmbiguousCreate::DoesNotAllocate,
            readiness: Some(true),
            reported: [
                Reported::Endpoint,
                Reported::ServedModel,
                Reported::OwnerTag,
                Reported::RequestId,
            ]
            .into_iter()
            .collect(),
            next_create: None,
            next_stop: None,
            resources: Vec::new(),
            incarnations: 0,
            allocations: 0,
            submits: 0,
            stop_calls: 0,
            lists: 0,
        }
    }

    /// Resources this provider actually created. The number a bill would be based on.
    pub const fn allocations(&self) -> u32 {
        self.allocations
    }
    /// Create mutations submitted, whether or not they allocated anything.
    pub const fn submits(&self) -> u32 {
        self.submits
    }
    /// Stop mutations submitted.
    pub const fn stops(&self) -> u32 {
        self.stop_calls
    }
    /// Listing calls. Listing never changes any of the counters above.
    pub const fn lists(&self) -> u32 {
        self.lists
    }

    /// Whether repeating a request id is guaranteed to allocate at most once.
    pub fn idempotency(&mut self, support: Idempotency) {
        self.idempotency = support;
    }
    /// Whether the next listings claim to be complete.
    pub fn inventory_completeness(&mut self, completeness: Completeness) {
        self.completeness = completeness;
    }
    /// What a create whose answer is lost actually does.
    pub fn ambiguous_create(&mut self, effect: AmbiguousCreate) {
        self.ambiguous = effect;
    }
    /// What readiness the provider reports. `None` means it reports none at all.
    pub fn report_readiness(&mut self, readiness: Option<bool>) {
        self.readiness = readiness;
    }
    fn set(&mut self, field: Reported, on: bool) {
        if on {
            self.reported.insert(field);
        } else {
            self.reported.remove(&field);
        }
    }
    /// Whether the provider reports an address.
    pub fn report_endpoint(&mut self, report: bool) {
        self.set(Reported::Endpoint, report);
    }
    /// Whether the provider reports which model is served.
    pub fn report_served_model(&mut self, report: bool) {
        self.set(Reported::ServedModel, report);
    }
    /// Whether the provider carries the owner and epoch tags back.
    pub fn tag_owner(&mut self, tag: bool) {
        self.set(Reported::OwnerTag, tag);
    }
    /// Whether a listed resource carries the idempotency key it was created with.
    ///
    /// A provider that does not is still able to list completely, which is why completeness
    /// alone cannot be used to conclude that nothing was created for a given request.
    pub fn echo_request_id(&mut self, echo: bool) {
        self.set(Reported::RequestId, echo);
    }
    /// Forces the dispatch of the next create only.
    pub fn next_create(&mut self, dispatch: Dispatch) {
        self.next_create = Some(dispatch);
    }
    /// Forces the dispatch of the next stop only.
    pub fn next_stop(&mut self, dispatch: Dispatch) {
        self.next_stop = Some(dispatch);
    }

    /// Removes every resource under a name, as an out-of-band deletion would.
    pub fn vanish(&mut self, name: &Identifier) {
        self.resources.retain(|stored| &stored.key.name != name);
    }

    /// Marks every resource under a name terminated, with the provider still listing it.
    pub fn terminate(&mut self, name: &Identifier) {
        for stored in &mut self.resources {
            if &stored.key.name == name {
                stored.terminated = true;
            }
        }
    }

    /// Hands a name to a new resource belonging to somebody else.
    ///
    /// This is the case a name cannot fence: the same name, a different resource, a different
    /// owner. It is not an allocation of ours and does not move the allocation counter.
    pub fn reuse_name(&mut self, name: &Identifier, owner: &Identifier) {
        self.incarnations = self.incarnations.saturating_add(1);
        let incarnation = self.incarnations;
        self.resources.push(Stored {
            key: ResourceKey {
                provider: self.provider.clone(),
                account: self.account.clone(),
                name: name.clone(),
                incarnation: Identifier::generated(format!("fake-incarnation-{incarnation}")),
            },
            request_id: Identifier::generated(format!("foreign-request-{incarnation}")),
            owner: owner.clone(),
            epoch: 0,
            terminated: false,
        });
    }

    /// Retags every resource under a name with a new owner and epoch, as a takeover would.
    pub fn retag(&mut self, name: &Identifier, owner: &Identifier, epoch: u64) {
        for stored in &mut self.resources {
            if &stored.key.name == name {
                stored.owner = owner.clone();
                stored.epoch = epoch;
            }
        }
    }

    fn allocate(&mut self, request: &CreateRequest) -> ObservedResource {
        if self.idempotency == Idempotency::Honoured
            && let Some(existing) = self
                .resources
                .iter()
                .find(|stored| stored.request_id == request.request_id)
        {
            return self.describe(&existing.clone());
        }
        self.incarnations = self.incarnations.saturating_add(1);
        self.allocations = self.allocations.saturating_add(1);
        let incarnation = self.incarnations;
        let stored = Stored {
            key: ResourceKey {
                provider: self.provider.clone(),
                account: self.account.clone(),
                name: request.spec.resource_name.clone(),
                incarnation: Identifier::generated(format!("fake-incarnation-{incarnation}")),
            },
            request_id: request.request_id.clone(),
            owner: request.owner.clone(),
            epoch: request.epoch,
            terminated: false,
        };
        self.resources.push(stored.clone());
        self.describe(&stored)
    }

    fn reports(&self, field: Reported) -> bool {
        self.reported.contains(&field)
    }

    fn describe(&self, stored: &Stored) -> ObservedResource {
        let state = if stored.terminated {
            ProviderState::Terminated
        } else if self.readiness == Some(true) {
            ProviderState::Running
        } else {
            ProviderState::Pending
        };
        ObservedResource {
            key: stored.key.clone(),
            state,
            ready: if stored.terminated {
                Some(false)
            } else {
                self.readiness
            },
            endpoint: self
                .reports(Reported::Endpoint)
                .then(|| format!("http://fake.invalid/{}", stored.key.incarnation)),
            served_model: self
                .reports(Reported::ServedModel)
                .then(|| Identifier::generated("fake-served-model")),
            owner: self
                .reports(Reported::OwnerTag)
                .then(|| stored.owner.clone()),
            epoch: self.reports(Reported::OwnerTag).then_some(stored.epoch),
            request_id: self
                .reports(Reported::RequestId)
                .then(|| stored.request_id.clone()),
        }
    }
}

impl HostingProvider for FakeProvider {
    fn create(&mut self, request: &CreateRequest) -> CreateOutcome {
        self.submits = self.submits.saturating_add(1);
        match self.next_create.take().unwrap_or(Dispatch::Accepted) {
            Dispatch::Accepted => {
                let resource = self.allocate(request);
                CreateOutcome {
                    dispatch: Dispatch::Accepted,
                    resource: Some(resource),
                }
            }
            Dispatch::Unknown => {
                if self.ambiguous == AmbiguousCreate::Allocates {
                    let _unheard = self.allocate(request);
                }
                CreateOutcome {
                    dispatch: Dispatch::Unknown,
                    resource: None,
                }
            }
            dispatch @ (Dispatch::NotSent | Dispatch::Rejected) => CreateOutcome {
                dispatch,
                resource: None,
            },
        }
    }

    fn stop(&mut self, key: &ResourceKey, _epoch: u64) -> StopOutcome {
        self.stop_calls = self.stop_calls.saturating_add(1);
        let dispatch = self.next_stop.take().unwrap_or(Dispatch::Accepted);
        if dispatch == Dispatch::Accepted {
            self.resources.retain(|stored| &stored.key != key);
            let stop = self.stop_calls;
            return StopOutcome {
                dispatch,
                evidence: Some(Identifier::generated(format!("fake-stop-{stop}"))),
            };
        }
        StopOutcome {
            dispatch,
            evidence: None,
        }
    }

    fn inventory(&mut self) -> Inventory {
        self.lists = self.lists.saturating_add(1);
        Inventory {
            completeness: self.completeness,
            resources: self
                .resources
                .iter()
                .map(|stored| self.describe(stored))
                .collect(),
        }
    }

    fn honours_idempotency_key(&self) -> bool {
        self.idempotency == Idempotency::Honoured
    }
}
