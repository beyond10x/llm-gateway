//! The owned-resource hosting controller: one scope, one lease per deployment, one record.

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    ComputeAuthorization, CreateRequest, DeploymentSpec, Dispatch, HostingPolicy, HostingProvider,
    Identifier, Inventory, Lease, LeaseRegistry, ObservedResource, Phase, ProviderState,
    ProvisionedDeployment, ResourceKey, StopReason, error::HostingError,
};

/// Evidence that a resource is absent from a listing that claimed to be complete.
pub const EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY: &str = "inventory-complete-absent";
/// Evidence that the provider itself reported the resource terminated.
pub const EVIDENCE_PROVIDER_TERMINATED: &str = "provider-terminated";

/// Everything durable about one deployment.
///
/// [`Controller::snapshot`] returns these and [`Controller::restore`] takes them back; that pair
/// is what a restart is, with no storage in the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentRecord {
    pub deployment: Identifier,
    pub phase: Phase,
    /// What was asked for.
    pub requested: DeploymentSpec,
    /// The budget reservation this resource is admitted against.
    pub authorization: ComputeAuthorization,
    /// The idempotency key, recorded before the create mutation was submitted.
    pub request_id: Identifier,
    /// What the provider reported. `None` until a provider answer exists.
    pub observed: Option<ProvisionedDeployment>,
    /// The claim this controller believes it holds. Cleared by a restart.
    pub lease: Option<Lease>,
    /// Whether a client is attached. Purely informational: it is not a lifecycle fact.
    pub connected: bool,
    /// Why a stop is owed. `None` when none is.
    pub stop_reason: Option<StopReason>,
    /// The evidence that discharged the obligation. `None` while it is open.
    pub stop_evidence: Option<Identifier>,
    /// When the create mutation was submitted, which is when the time ceiling starts.
    pub started_at_ms: Option<u64>,
}

/// Aggregate facts a caller needs without walking every record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingTotals {
    /// Records occupying a slot against the resource ceiling.
    pub active_count: usize,
    /// Records whose very existence is unresolved.
    pub unknown_count: usize,
    /// Deployments that owe a stop and a confirmation, in identifier order.
    ///
    /// Named and shaped like `llm-cost`'s `BudgetTotals::stop_required` because it is the same
    /// obligation seen from the other side of the seam.
    pub stop_required: Vec<Identifier>,
    /// Deployments whose obligation moved to a newer epoch, in identifier order.
    ///
    /// A `Disowned` record leaves `stop_required` without ever attaching evidence, because
    /// nothing stopped: the obligation was **transferred**, not discharged. Without this list
    /// that transfer is a disappearance, and an operator reconciling two controllers has no
    /// way to see which resources the older one handed over.
    pub transferred: Vec<Identifier>,
    pub at_capacity: bool,
}

/// One inspection of the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingView {
    pub now_ms: u64,
    pub policy: HostingPolicy,
    pub deployments: Vec<DeploymentRecord>,
    pub totals: HostingTotals,
}

/// What a command did, in the terms the caller can act on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostingReceipt {
    /// The dispatch of the provider mutation this command submitted, if it submitted one.
    pub dispatch: Option<Dispatch>,
    /// The claim this command granted or extended.
    pub lease: Option<Lease>,
    /// The phase the affected deployment is in afterwards.
    pub phase: Option<Phase>,
}

/// One command applied to the controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostingCommand {
    /// Observe elapsed obligations without contacting the provider at all.
    Tick,
    /// Check a specification against the scope. Allocates nothing, contacts nobody.
    Validate { spec: DeploymentSpec },
    /// Record an authorized deployment. Allocates nothing.
    Declare {
        spec: DeploymentSpec,
        request_id: Identifier,
        authorization: ComputeAuthorization,
    },
    /// Take ownership of a deployment at a new epoch.
    Acquire { deployment: Identifier },
    /// Extend the current claim without changing its epoch.
    Renew { deployment: Identifier },
    /// Give up the claim. Not shutdown.
    Release { deployment: Identifier },
    /// Submit the create mutation.
    Provision { deployment: Identifier },
    /// Resubmit the recorded request id after an unknown create. Refused unless the provider
    /// documents that a repeated request id allocates at most once.
    Retry { deployment: Identifier },
    /// Reconcile every record against the provider's listing. Allocates nothing.
    Observe,
    /// Attach a client. Changes no lifecycle fact.
    Connect { deployment: Identifier },
    /// Detach a client. Not shutdown.
    Disconnect { deployment: Identifier },
    /// Record that the resource must be stopped. Contacts nobody.
    RequestStop { deployment: Identifier },
    /// Submit the stop mutation.
    Stop { deployment: Identifier },
    /// Record external evidence that the resource stopped.
    ConfirmStopped {
        deployment: Identifier,
        evidence: Identifier,
    },
    /// Withdraw a deployment that was never submitted.
    Cancel { deployment: Identifier },
}

/// One hosting controller over one scope.
#[derive(Debug)]
pub struct Controller {
    policy: HostingPolicy,
    leases: Arc<LeaseRegistry>,
    records: BTreeMap<Identifier, DeploymentRecord>,
    now_ms: u64,
}

impl Controller {
    /// Opens a controller over an empty scope.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::InvalidPolicy`] when the policy declares an unusable ceiling.
    pub fn new(
        policy: HostingPolicy,
        leases: Arc<LeaseRegistry>,
        now_ms: u64,
    ) -> Result<Self, HostingError> {
        policy.validate()?;
        Ok(Self {
            policy,
            leases,
            records: BTreeMap::new(),
            now_ms,
        })
    }

    /// Reopens a controller over durable records after a restart.
    ///
    /// Two things happen and both are the point. Every claim is dropped, so the restarted
    /// controller must reacquire — at a new epoch — before it may mutate anything; and every
    /// liveness observation is dropped, so a readiness seen before the restart cannot be
    /// mistaken for a readiness now. Identity, phase and open obligations survive intact: a
    /// restart is not a discharge.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::InvalidPolicy`] when the policy declares an unusable ceiling,
    /// [`HostingError::InvalidSpec`] when a restored record's specification belongs to another
    /// scope, [`HostingError::ForeignResource`] when a restored record claims a resource
    /// identity outside this scope, and [`HostingError::ClockReversed`] when `now_ms` is
    /// earlier than an instant the restored records already witnessed.
    ///
    /// That last one closes the door the in-memory clock guard cannot watch. `apply` refuses a
    /// clock that moves backwards, but the guard lives on the instance, so a restart would
    /// otherwise reset it — and a controller reopened at an earlier instant would un-expire
    /// leases and un-fire time ceilings. The floor is the latest instant the snapshot itself
    /// witnesses. A snapshot holding no records witnesses nothing, so it constrains nothing,
    /// and this refusal does not claim otherwise.
    pub fn restore(
        policy: HostingPolicy,
        leases: Arc<LeaseRegistry>,
        snapshot: Vec<DeploymentRecord>,
        now_ms: u64,
    ) -> Result<Self, HostingError> {
        policy.validate()?;
        let mut records = BTreeMap::new();
        for mut record in snapshot {
            if record.requested.provider != policy.provider
                || record.requested.account != policy.account
            {
                return Err(HostingError::InvalidSpec);
            }
            if let Some(observed) = &record.observed
                && !key_in_scope(&observed.key, &policy)
            {
                return Err(HostingError::ForeignResource);
            }
            if now_ms < witnessed(&record) {
                return Err(HostingError::ClockReversed);
            }
            record.lease = None;
            record.connected = false;
            record.observed = record
                .observed
                .as_ref()
                .map(ProvisionedDeployment::forgetting_liveness);
            records.insert(record.deployment.clone(), record);
        }
        Ok(Self {
            policy,
            leases,
            records,
            now_ms,
        })
    }

    /// Applies one command using a trusted Unix-millisecond clock.
    ///
    /// Time cannot move backwards. Every call first observes elapsed obligations — the time
    /// ceiling and lease expiry — even when the requested command is then refused, exactly as
    /// the budget ledger does.
    ///
    /// # Errors
    ///
    /// Returns the [`HostingError`] the command was refused with. A refusal never leaves a
    /// partially applied change and never discharges an obligation.
    pub fn apply(
        &mut self,
        now_ms: u64,
        provider: &mut dyn HostingProvider,
        command: HostingCommand,
    ) -> Result<HostingReceipt, HostingError> {
        if now_ms < self.now_ms {
            return Err(HostingError::ClockReversed);
        }
        self.now_ms = now_ms;
        self.elapse();
        match command {
            HostingCommand::Tick => Ok(HostingReceipt::default()),
            HostingCommand::Validate { spec } => {
                spec.validate(&self.policy)?;
                Ok(HostingReceipt::default())
            }
            HostingCommand::Declare {
                spec,
                request_id,
                authorization,
            } => self.declare(spec, request_id, authorization),
            HostingCommand::Acquire { deployment } => self.acquire(&deployment),
            HostingCommand::Renew { deployment } => self.renew(&deployment),
            HostingCommand::Release { deployment } => self.release(&deployment),
            HostingCommand::Provision { deployment } => self.provision(provider, &deployment),
            HostingCommand::Retry { deployment } => self.retry(provider, &deployment),
            HostingCommand::Observe => {
                let inventory = provider.inventory();
                self.reconcile(&inventory);
                Ok(HostingReceipt::default())
            }
            HostingCommand::Connect { deployment } => self.attach(&deployment, true),
            HostingCommand::Disconnect { deployment } => self.attach(&deployment, false),
            HostingCommand::RequestStop { deployment } => self.request_stop(&deployment),
            HostingCommand::Stop { deployment } => self.stop(provider, &deployment),
            HostingCommand::ConfirmStopped {
                deployment,
                evidence,
            } => self.confirm_stopped(&deployment, evidence),
            HostingCommand::Cancel { deployment } => self.cancel(&deployment),
        }
    }

    /// Inspects the controller. Contacts nobody.
    pub fn view(&self) -> HostingView {
        let deployments: Vec<_> = self.records.values().cloned().collect();
        let active_count = deployments
            .iter()
            .filter(|record| record.phase.occupies_a_slot())
            .count();
        HostingView {
            now_ms: self.now_ms,
            policy: self.policy.clone(),
            totals: HostingTotals {
                active_count,
                unknown_count: deployments
                    .iter()
                    .filter(|record| record.phase == Phase::Uncertain)
                    .count(),
                stop_required: deployments
                    .iter()
                    .filter(|record| record.phase.owes_a_stop())
                    .map(|record| record.deployment.clone())
                    .collect(),
                transferred: deployments
                    .iter()
                    .filter(|record| record.phase == Phase::Disowned)
                    .map(|record| record.deployment.clone())
                    .collect(),
                at_capacity: active_count >= self.policy.max_active as usize,
            },
            deployments,
        }
    }

    /// The durable records, as a restart would read them back.
    pub fn snapshot(&self) -> Vec<DeploymentRecord> {
        self.records.values().cloned().collect()
    }

    // --- obligations that accrue with time -------------------------------------------------

    fn elapse(&mut self) {
        let now_ms = self.now_ms;
        let policy = &self.policy;
        // There is deliberately no terminal skip here. `require_stop` refuses every terminal
        // phase already — none of them has an edge to `StopRequired` — so a skip in front of it
        // decides nothing, and a guard that decides nothing reads as protection while being
        // checked by nothing. The same rule that keeps a scope check out of `Stop` keeps this
        // one out of `elapse`.
        for record in self.records.values_mut() {
            // The time ceiling is checked before lease expiry: a resource past its deadline
            // must be stopped whoever owns it, while an expired lease only ends a claim.
            if let Some(started) = record.started_at_ms {
                // One expression, published once. A policy tightened across a restart lowers
                // the deadline of a record declared under the looser one.
                let deadline =
                    started.saturating_add(record.requested.effective_lifetime_ms(policy));
                if now_ms > deadline {
                    require_stop(record, StopReason::LifetimeExceeded);
                    continue;
                }
            }
            if let Some(lease) = &record.lease
                && !lease.live_at(now_ms)
            {
                require_stop(record, StopReason::LeaseExpired);
            }
        }
    }

    // --- commands ---------------------------------------------------------------------------

    fn declare(
        &mut self,
        spec: DeploymentSpec,
        request_id: Identifier,
        authorization: ComputeAuthorization,
    ) -> Result<HostingReceipt, HostingError> {
        spec.validate(&self.policy)?;
        if authorization.ledger != self.policy.ledger {
            return Err(HostingError::Unauthorized);
        }
        if self.records.contains_key(&spec.id) {
            return Err(HostingError::Duplicate);
        }
        let deployment = spec.id.clone();
        self.records.insert(
            deployment.clone(),
            DeploymentRecord {
                deployment,
                phase: Phase::Declared,
                requested: spec,
                authorization,
                request_id,
                observed: None,
                lease: None,
                connected: false,
                stop_reason: None,
                stop_evidence: None,
                started_at_ms: None,
            },
        );
        Ok(HostingReceipt {
            phase: Some(Phase::Declared),
            ..HostingReceipt::default()
        })
    }

    fn acquire(&mut self, deployment: &Identifier) -> Result<HostingReceipt, HostingError> {
        if !self.records.contains_key(deployment) {
            return Err(HostingError::Missing);
        }
        let lease = self.leases.acquire(
            &self.policy.controller,
            deployment,
            self.now_ms,
            self.policy.lease_ms,
        )?;
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        record.lease = Some(lease.clone());
        Ok(HostingReceipt {
            lease: Some(lease),
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    fn renew(&mut self, deployment: &Identifier) -> Result<HostingReceipt, HostingError> {
        let held = self
            .records
            .get(deployment)
            .ok_or(HostingError::Missing)?
            .lease
            .clone()
            .ok_or(HostingError::LeaseLost)?;
        let lease = self.leases.renew(
            &self.policy.controller,
            deployment,
            held.epoch,
            self.now_ms,
            self.policy.lease_ms,
        )?;
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        record.lease = Some(lease.clone());
        Ok(HostingReceipt {
            lease: Some(lease),
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    fn release(&mut self, deployment: &Identifier) -> Result<HostingReceipt, HostingError> {
        let held = self
            .records
            .get(deployment)
            .ok_or(HostingError::Missing)?
            .lease
            .clone()
            .ok_or(HostingError::LeaseLost)?;
        self.leases
            .release(&self.policy.controller, deployment, held.epoch)?;
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        record.lease = None;
        Ok(HostingReceipt {
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    /// Refuses unless this controller's recorded claim is the registry's current one.
    ///
    /// Three facts would have to hold for the registry's current lease to be ours, and only one
    /// of them is an independent check. Once `epoch(deployment) <= held.epoch` has passed, the
    /// registry's own invariant — a granted epoch is strictly monotonic and a lease is stored
    /// together with the epoch it was granted at, so `current().epoch == epoch()` whenever a
    /// lease is held — already forces `current.epoch == held.epoch` and, because one epoch is
    /// granted to one owner, `current.owner == self.policy.controller`. Those two comparisons
    /// were written here and could not be tripped by anything. They are asserted where the
    /// invariant actually lives instead:
    /// `lease::tests::a_held_lease_always_carries_the_registrys_current_epoch_and_its_grantee`.
    ///
    /// What is left is the one fact the registry does not decide for us: whether the claim is
    /// still live at *this* controller's clock. A claim that simply ran out with nobody taking
    /// over leaves the epoch untouched, so it is invisible to the comparison above.
    fn fence(&self, deployment: &Identifier) -> Result<u64, HostingError> {
        let held = self
            .records
            .get(deployment)
            .ok_or(HostingError::Missing)?
            .lease
            .as_ref()
            .ok_or(HostingError::LeaseLost)?;
        if self.leases.epoch(deployment) > held.epoch {
            return Err(HostingError::StaleEpoch);
        }
        let current = self
            .leases
            .current(deployment)
            .ok_or(HostingError::LeaseLost)?;
        if !current.live_at(self.now_ms) {
            return Err(HostingError::LeaseLost);
        }
        Ok(held.epoch)
    }

    fn provision(
        &mut self,
        provider: &mut dyn HostingProvider,
        deployment: &Identifier,
    ) -> Result<HostingReceipt, HostingError> {
        let epoch = self.fence(deployment)?;
        let record = self.records.get(deployment).ok_or(HostingError::Missing)?;
        if record.phase != Phase::Declared {
            return Err(HostingError::WrongPhase);
        }
        // The ceiling is answered from records this controller already holds, before anything
        // is submitted: a refusal here has cost nothing and allocated nothing.
        if self.view().totals.at_capacity {
            return Err(HostingError::CapacityExceeded);
        }
        let request = CreateRequest {
            spec: record.requested.clone(),
            request_id: record.request_id.clone(),
            owner: self.policy.controller.clone(),
            epoch,
        };
        let outcome = provider.create(&request);
        let now_ms = self.now_ms;
        let policy = self.policy.clone();
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        record.started_at_ms = Some(now_ms);
        let phase = settle_create(
            record,
            &policy,
            outcome.dispatch,
            outcome.resource.as_ref(),
            now_ms,
        );
        Ok(HostingReceipt {
            dispatch: Some(outcome.dispatch),
            phase: Some(phase),
            lease: record.lease.clone(),
        })
    }

    fn retry(
        &mut self,
        provider: &mut dyn HostingProvider,
        deployment: &Identifier,
    ) -> Result<HostingReceipt, HostingError> {
        let epoch = self.fence(deployment)?;
        let record = self.records.get(deployment).ok_or(HostingError::Missing)?;
        if record.phase != Phase::Uncertain {
            return Err(HostingError::WrongPhase);
        }
        if !provider.honours_idempotency_key() {
            // Resubmitting would risk a second billed resource for one authorization. The
            // obligation stays open and reconciliation is the only remaining route.
            return Err(HostingError::IdempotencyUnsupported);
        }
        let request = CreateRequest {
            spec: record.requested.clone(),
            request_id: record.request_id.clone(),
            owner: self.policy.controller.clone(),
            epoch,
        };
        let outcome = provider.create(&request);
        let now_ms = self.now_ms;
        let policy = self.policy.clone();
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        let phase = settle_create(
            record,
            &policy,
            outcome.dispatch,
            outcome.resource.as_ref(),
            now_ms,
        );
        Ok(HostingReceipt {
            dispatch: Some(outcome.dispatch),
            phase: Some(phase),
            lease: record.lease.clone(),
        })
    }

    /// Attaches or detaches a client.
    ///
    /// Neither direction is a lifecycle event. Detaching in particular changes nothing about
    /// the resource, its phase or its obligation: a provider keeps billing an idle GPU, and
    /// "the client went away" has never been evidence that anything stopped.
    fn attach(
        &mut self,
        deployment: &Identifier,
        connected: bool,
    ) -> Result<HostingReceipt, HostingError> {
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        record.connected = connected;
        Ok(HostingReceipt {
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    fn request_stop(&mut self, deployment: &Identifier) -> Result<HostingReceipt, HostingError> {
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        // The phase write is the whole command, so its refusal is the command's refusal. A
        // command that reports `Ok` for a change the table forbade has recorded no obligation
        // and told nobody — the silent no-op this contract exists to make impossible — and
        // `Declared -> StopRequired` is exactly such a pair: nothing can be owed before a
        // mutation was submitted. `Cancel` refuses the mirror case with the same code.
        if !require_stop(record, StopReason::OperatorRequest) {
            return Err(HostingError::WrongPhase);
        }
        Ok(HostingReceipt {
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    fn stop(
        &mut self,
        provider: &mut dyn HostingProvider,
        deployment: &Identifier,
    ) -> Result<HostingReceipt, HostingError> {
        // Fencing is the outermost guard: a controller that has been superseded learns that
        // before it learns anything about its own record's phase.
        let epoch = self.fence(deployment)?;
        let record = self.records.get(deployment).ok_or(HostingError::Missing)?;
        if record.phase.terminal() {
            return Err(HostingError::WrongPhase);
        }
        if record.stop_reason == Some(StopReason::OwnershipLost) {
            // The provider says this resource is somebody else's. Stopping it would destroy
            // another owner's compute, which is the failure the whole contract exists to
            // prevent; the obligation stays open for a human to reconcile.
            return Err(HostingError::ForeignResource);
        }
        let Some(observed) = record.observed.as_ref() else {
            // Nothing to address the mutation to. The obligation stays open; only a listing
            // that can answer for this request id can resolve it.
            return Err(HostingError::AmbiguousMutation);
        };
        // There is deliberately no scope check on `observed.key` here. An out-of-scope
        // identity cannot reach a record: `settle_create` refuses to adopt one, `adopt`
        // refuses to take one from a listing, and `restore` refuses a snapshot carrying one.
        // A guard nothing can trip is the same defect as a refusal nothing can emit — it
        // reads as protection and is not checked by anything — so it is not written.
        let key = observed.key.clone();
        let outcome = provider.stop(&key, epoch);
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        match (outcome.dispatch, outcome.evidence) {
            (Dispatch::Accepted, Some(evidence)) => {
                discharge(record, evidence);
            }
            // Confirmed neither by the provider nor by anyone else. Still owed.
            _ => {
                require_stop(record, StopReason::OperatorRequest);
            }
        }
        Ok(HostingReceipt {
            dispatch: Some(outcome.dispatch),
            phase: Some(record.phase),
            lease: record.lease.clone(),
        })
    }

    fn confirm_stopped(
        &mut self,
        deployment: &Identifier,
        evidence: Identifier,
    ) -> Result<HostingReceipt, HostingError> {
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        // `discharge` owns the guard, and this is the command that trips it. Asking
        // `may_become(Stopped)` here as well would duplicate the decision and leave the copy
        // inside `discharge` unreachable — a guard nothing can trip, which this contract treats
        // as the same defect as a refusal nothing can emit.
        if !discharge(record, evidence) {
            return Err(HostingError::WrongPhase);
        }
        Ok(HostingReceipt {
            phase: Some(record.phase),
            ..HostingReceipt::default()
        })
    }

    fn cancel(&mut self, deployment: &Identifier) -> Result<HostingReceipt, HostingError> {
        let record = self
            .records
            .get_mut(deployment)
            .ok_or(HostingError::Missing)?;
        // Cancellation releases an obligation that was never incurred. A submitted create,
        // even one whose answer was lost, is past that point.
        if !record.phase.may_become(Phase::Cancelled) {
            return Err(HostingError::WrongPhase);
        }
        record.phase = Phase::Cancelled;
        Ok(HostingReceipt {
            phase: Some(Phase::Cancelled),
            ..HostingReceipt::default()
        })
    }

    // --- reconciliation ----------------------------------------------------------------------

    fn reconcile(&mut self, inventory: &Inventory) {
        let now_ms = self.now_ms;
        let policy = self.policy.clone();
        for record in self.records.values_mut() {
            if record.phase.terminal() || record.phase == Phase::Declared {
                continue;
            }
            let held_epoch = record.lease.as_ref().map_or(0, |lease| lease.epoch);
            match record
                .observed
                .as_ref()
                .map(|observed| observed.key.clone())
            {
                Some(key) => match inventory.exact(&key) {
                    Some(resource) => adopt(record, resource, &policy, held_epoch, now_ms),
                    None if inventory.complete() => {
                        discharge(
                            record,
                            Identifier::generated(EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY),
                        );
                    }
                    // A partial listing carries no information. No transition at all.
                    None => (),
                },
                // No key yet: the only question a listing can answer is "was anything
                // created under my request id?", and it can only answer it when every
                // listed resource echoes one. Completeness alone does not promise that,
                // so a listing that cannot answer produces no transition — the same
                // silence a partial listing produces, for the same reason.
                None => match inventory.by_request(&record.request_id) {
                    Some(resource) => adopt(record, resource, &policy, held_epoch, now_ms),
                    None if inventory.answers_request_identity() => {
                        discharge(
                            record,
                            Identifier::generated(EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY),
                        );
                    }
                    None => (),
                },
            }
        }
    }
}

/// Records a provider answer to a create mutation.
fn settle_create(
    record: &mut DeploymentRecord,
    policy: &HostingPolicy,
    dispatch: Dispatch,
    resource: Option<&ObservedResource>,
    now_ms: u64,
) -> Phase {
    match (dispatch, resource) {
        (Dispatch::Accepted, Some(resource))
            if !key_in_scope(&resource.key, policy) || !owner_is_ours(resource, policy) =>
        {
            // The provider answered about a resource in another scope or labelled for another
            // owner. It is not recorded as this record's own — an answer that cannot be
            // accounted for is not an adoption — and the mutation we submitted may still have
            // created something, so the obligation stays open.
            if record.phase.may_become(Phase::Uncertain) {
                record.phase = Phase::Uncertain;
            }
            record_stop_reason(record, StopReason::OwnershipLost);
        }
        (Dispatch::Accepted, Some(resource)) => {
            if record.phase.may_become(Phase::Requested) {
                record.observed = Some(ProvisionedDeployment::from_observation(resource, now_ms));
                record.phase = Phase::Requested;
            }
        }
        (Dispatch::NotSent | Dispatch::Rejected, _) => {
            // The provider says nothing was allocated. That is an answer, so a record that
            // never left `Declared` is withdrawn. A record that already owes a stop is not:
            // a later refusal is not evidence about an earlier ambiguous create.
            if record.phase.may_become(Phase::Cancelled) {
                record.phase = Phase::Cancelled;
                record.started_at_ms = None;
            }
        }
        // Accepted without a resource says as little as Unknown does, and is treated as it.
        (Dispatch::Unknown, _) | (Dispatch::Accepted, None) => {
            if record.phase.may_become(Phase::Uncertain) {
                record.phase = Phase::Uncertain;
            }
            record_stop_reason(record, StopReason::AmbiguousMutation);
        }
    }
    record.phase
}

/// Takes an observed resource as this record's own, when it really is.
fn adopt(
    record: &mut DeploymentRecord,
    resource: &ObservedResource,
    policy: &HostingPolicy,
    held_epoch: u64,
    now_ms: u64,
) {
    if !key_in_scope(&resource.key, policy) {
        // Reached only through the request-id route, which searches the whole listing. A
        // resource in another scope is never this record's, whatever key it carries.
        require_stop(record, StopReason::OwnershipLost);
        return;
    }
    if let Some(owner) = &resource.owner
        && owner != &policy.controller
    {
        // Somebody else's tag. A strictly newer epoch is a takeover, so the obligation moves
        // with the ownership; anything else is a resource we cannot account for and must stop
        // mutating, which is an obligation that stays here. `Requested -> Uncertain` and
        // `Active -> Uncertain` are not edges of the table, so asking for `Uncertain` here
        // would be a silent no-op; `StopRequired` is reachable from every live phase and is
        // what "we stop mutating it" actually means.
        //
        // `ownership-lost` displaces whatever reason was already in the slot. It has to: an
        // operator request or an expired lease gets there first on the two ordinary sequences
        // that reach this line — `RequestStop`/`Observe`/`Stop`, and lease expiry, reacquisition
        // by another controller, `Observe`, `Stop` — and `Stop` reads this slot to decide
        // whether the resource is still ours to touch. See `record_stop_reason`.
        if resource.epoch.is_some_and(|epoch| epoch > held_epoch) {
            if record.phase.may_become(Phase::Disowned) {
                record.phase = Phase::Disowned;
            }
        } else {
            require_stop(record, StopReason::OwnershipLost);
        }
        return;
    }
    record.observed = Some(ProvisionedDeployment::from_observation(resource, now_ms));
    if resource.state == ProviderState::Terminated {
        discharge(record, Identifier::generated(EVIDENCE_PROVIDER_TERMINATED));
        return;
    }
    // The one place the reason slot is cleared; `record_stop_reason` is the only place it is
    // written. Ownership is no longer in question here: the provider listed this resource under
    // this record's exact key and did not label it for anybody else.
    if record.phase.may_become(Phase::Active) {
        record.phase = Phase::Active;
        // The resource has been seen under this record's exact key, so whatever ambiguity or
        // lost ownership opened a reason is resolved. An obligation that survived — a stop
        // already required — never reaches here, because StopRequired cannot become Active.
        record.stop_reason = None;
    }
}

/// The latest instant one durable record already witnessed.
fn witnessed(record: &DeploymentRecord) -> u64 {
    record.started_at_ms.unwrap_or(0).max(
        record
            .observed
            .as_ref()
            .map_or(0, |observed| observed.observed_at_ms),
    )
}

/// Whether a resource identity is one this controller's scope could own.
///
/// The provider and account of a key are **not** the provider's to report: the controller chose
/// them when it configured the seam. A provider adapter that answers with another provider or
/// account is answering about a resource outside this scope, and this controller must neither
/// record it as its own nor ever address a mutation to it.
fn key_in_scope(key: &ResourceKey, policy: &HostingPolicy) -> bool {
    key.provider == policy.provider && key.account == policy.account
}

/// Whether an owner label, if the provider carries one, names this controller.
fn owner_is_ours(resource: &ObservedResource, policy: &HostingPolicy) -> bool {
    resource
        .owner
        .as_ref()
        .is_none_or(|owner| owner == &policy.controller)
}

/// The one writer of `stop_reason`, and the one place its precedence rule is stated.
///
/// **First writer wins, except that `OwnershipLost` always wins.** The reason that opened an
/// obligation is the one that explains it, and a later operator request or an expired lease says
/// nothing new about a resource already owed. `OwnershipLost` is not another such reason: it is
/// the only one that is a statement about *who owns the resource* rather than about why this
/// controller wants it stopped, and it is the fact [`Controller::stop`]'s `foreign-resource`
/// refusal is keyed on.
///
/// A refusal keyed on a slot several writers share is only as strong as that slot's precedence,
/// and this one had none. `Stop`'s guard read `OwnershipLost` while first-writer-wins meant the
/// tag was simply dropped whenever an operator request or a lease expiry had filled the slot
/// first — so a resource the provider said belonged to another controller was stopped, and the
/// record filed the destruction as its own discharged obligation. Both halves of the rule are
/// asserted, over every pair of published reasons, by
/// `tests::the_first_reason_is_kept_except_that_ownership_lost_always_wins`.
///
/// The slot is cleared in exactly one place — `adopt`, when the resource is seen again under
/// this record's own key and its ownership is therefore no longer in question.
fn record_stop_reason(record: &mut DeploymentRecord, reason: StopReason) {
    if record.stop_reason.is_none() || reason == StopReason::OwnershipLost {
        record.stop_reason = Some(reason);
    }
}

/// Opens or keeps a stop obligation. Returns whether the obligation is now open.
///
/// `false` means the table refused the change: the phase is terminal and nothing was recorded.
/// Callers that are commands turn that into [`HostingError::WrongPhase`] rather than reporting
/// success for a write that did not happen; callers that are reconciliation have nobody to tell.
fn require_stop(record: &mut DeploymentRecord, reason: StopReason) -> bool {
    if !record.phase.may_become(Phase::StopRequired) {
        return false;
    }
    record.phase = Phase::StopRequired;
    record_stop_reason(record, reason);
    true
}

/// Closes a stop obligation, and only ever with evidence. Returns whether it closed.
///
/// `false` means the table refused `-> Stopped` from this record's phase, so no evidence was
/// attached either. `ConfirmStopped` is the command that trips it.
fn discharge(record: &mut DeploymentRecord, evidence: Identifier) -> bool {
    if !record.phase.may_become(Phase::Stopped) {
        return false;
    }
    record.phase = Phase::Stopped;
    record.stop_evidence = Some(evidence);
    true
}

#[cfg(test)]
mod tests {
    use super::{DeploymentRecord, Phase, StopReason, discharge, record_stop_reason, require_stop};
    use crate::{ComputeAuthorization, DeploymentSpec, Identifier};

    fn id(value: &str) -> Identifier {
        Identifier::new(value).expect("identifier")
    }

    fn record(phase: Phase, stop_reason: Option<StopReason>) -> DeploymentRecord {
        DeploymentRecord {
            deployment: id("d1"),
            phase,
            requested: DeploymentSpec {
                id: id("d1"),
                provider: id("fake"),
                account: id("account-1"),
                model: id("requested-model"),
                image: id("requested-image"),
                resource_name: id("pool-slot"),
                requested_lifetime_ms: 5_000,
            },
            authorization: ComputeAuthorization {
                ledger: id("scope"),
                reservation: id("reservation-a"),
            },
            request_id: id("request-d1"),
            observed: None,
            lease: None,
            connected: false,
            stop_reason,
            stop_evidence: None,
            started_at_ms: None,
        }
    }

    /// The precedence rule, in **both** directions and over every ordered pair of published
    /// reasons — so a reason added to the vocabulary is covered without this test being edited.
    ///
    /// One direction is that an already-recorded reason survives a later one: the reason that
    /// opened an obligation is the one that explains it. The other is that `ownership-lost`
    /// displaces it anyway, because `Stop`'s `foreign-resource` refusal reads this slot to
    /// decide whether the resource is still ours to touch, and a rule keyed on a shared slot is
    /// only as strong as its precedence. Neither half was asserted before, and the missing half
    /// is what let an operator-requested stop destroy another controller's compute.
    #[test]
    fn the_first_reason_is_kept_except_that_ownership_lost_always_wins() {
        for &first in StopReason::ALL {
            for &second in StopReason::ALL {
                let mut record = record(Phase::Active, Some(first));
                record_stop_reason(&mut record, second);
                let expected = if second == StopReason::OwnershipLost {
                    second
                } else {
                    first
                };
                assert_eq!(
                    record.stop_reason,
                    Some(expected),
                    "{} recorded, then {} arrives",
                    first.code(),
                    second.code()
                );
            }
        }
    }

    #[test]
    fn an_empty_reason_slot_takes_whatever_reason_arrives_first() {
        for &reason in StopReason::ALL {
            let mut record = record(Phase::Active, None);
            record_stop_reason(&mut record, reason);
            assert_eq!(record.stop_reason, Some(reason), "{}", reason.code());
        }
    }

    /// The phases an obligation can be opened from, written out rather than derived from the
    /// table being tested, and the refusal reported rather than swallowed.
    ///
    /// `require_stop` returning `false` is what lets `RequestStop` refuse with `wrong-phase`
    /// instead of reporting `Ok` for a phase write the table forbade.
    #[test]
    fn opening_an_obligation_is_refused_and_reported_outside_the_four_live_phases() {
        let live = [
            Phase::Requested,
            Phase::Active,
            Phase::Uncertain,
            Phase::StopRequired,
        ];
        for &phase in Phase::ALL {
            let mut subject = record(phase, None);
            let opened = require_stop(&mut subject, StopReason::OperatorRequest);
            assert_eq!(opened, live.contains(&phase), "open from {phase:?}");
            if opened {
                assert_eq!(subject.phase, Phase::StopRequired);
                assert_eq!(subject.stop_reason, Some(StopReason::OperatorRequest));
            } else {
                assert_eq!(subject.phase, phase, "a refused write changed the phase");
                assert_eq!(
                    subject.stop_reason, None,
                    "a refused write recorded a reason"
                );
            }
        }
    }

    /// The same, for the closing half: `ConfirmStopped` trips this guard and reports it.
    #[test]
    fn closing_an_obligation_is_refused_and_reported_outside_the_four_live_phases() {
        let live = [
            Phase::Requested,
            Phase::Active,
            Phase::Uncertain,
            Phase::StopRequired,
        ];
        for &phase in Phase::ALL {
            let mut subject = record(phase, None);
            let closed = discharge(&mut subject, id("invoice-line-7"));
            assert_eq!(closed, live.contains(&phase), "close from {phase:?}");
            if closed {
                assert_eq!(subject.phase, Phase::Stopped);
                assert_eq!(subject.stop_evidence, Some(id("invoice-line-7")));
            } else {
                assert_eq!(subject.phase, phase, "a refused write changed the phase");
                assert_eq!(
                    subject.stop_evidence, None,
                    "a refused write attached evidence"
                );
            }
        }
    }
}
