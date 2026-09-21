//! Ownership leases and the fencing epoch.
//!
//! Two controllers must never own one billed resource. The lease is how that is decided and the
//! epoch is how a controller that has already lost is stopped from acting on a stale belief.

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

use crate::{Identifier, error::HostingError};

/// One controller's claim on one deployment.
///
/// The `epoch` is strictly monotonic per deployment and is never reused, including across the
/// original owner's restart. It is the fencing token: a mutation carrying an epoch below the
/// registry's current one is refused before it reaches the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub deployment: Identifier,
    pub owner: Identifier,
    pub epoch: u64,
    /// Inclusive. Expiry ends the *claim*; it says nothing about the resource.
    pub expires_at_ms: u64,
}

impl Lease {
    /// Whether the claim is still live at this instant.
    pub const fn live_at(&self, now_ms: u64) -> bool {
        now_ms <= self.expires_at_ms
    }
}

#[derive(Debug)]
struct Entry {
    /// Highest epoch ever granted for this deployment. Never decreases, never resets.
    epoch: u64,
    held: Option<Lease>,
}

/// The authority that decides who owns a deployment.
///
/// In a deployed system this is a durable, linearizable store. Here it is an in-process,
/// mutex-serialized map, which is enough to decide the contract's question — that exactly one
/// of any number of simultaneous acquirers is granted the lease — without any I/O.
#[derive(Debug, Default)]
pub struct LeaseRegistry {
    entries: Mutex<BTreeMap<Identifier, Entry>>,
}

impl LeaseRegistry {
    fn entries(&self) -> std::sync::MutexGuard<'_, BTreeMap<Identifier, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes ownership of a deployment at a new epoch.
    ///
    /// Granted when nobody holds the lease, when the current holder's claim has expired, or
    /// when the same owner reclaims it — a restarted controller is a new incarnation and gets a
    /// new epoch, which is what fences its own pre-restart self.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::LeaseHeld`] when a different owner holds a live claim.
    pub fn acquire(
        &self,
        owner: &Identifier,
        deployment: &Identifier,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<Lease, HostingError> {
        let mut entries = self.entries();
        let entry = entries.entry(deployment.clone()).or_insert(Entry {
            epoch: 0,
            held: None,
        });
        if let Some(held) = &entry.held
            && held.live_at(now_ms)
            && &held.owner != owner
        {
            return Err(HostingError::LeaseHeld);
        }
        entry.epoch = entry.epoch.saturating_add(1);
        let lease = Lease {
            deployment: deployment.clone(),
            owner: owner.clone(),
            epoch: entry.epoch,
            expires_at_ms: now_ms.saturating_add(lease_ms),
        };
        entry.held = Some(lease.clone());
        Ok(lease)
    }

    /// Extends a claim this owner already holds, keeping its epoch.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::StaleEpoch`] when a newer epoch has been granted, and
    /// [`HostingError::LeaseLost`] when this owner holds no live claim.
    pub fn renew(
        &self,
        owner: &Identifier,
        deployment: &Identifier,
        epoch: u64,
        now_ms: u64,
        lease_ms: u64,
    ) -> Result<Lease, HostingError> {
        let mut entries = self.entries();
        let entry = entries.get_mut(deployment).ok_or(HostingError::LeaseLost)?;
        if entry.epoch > epoch {
            return Err(HostingError::StaleEpoch);
        }
        let held = entry.held.as_mut().ok_or(HostingError::LeaseLost)?;
        if &held.owner != owner || held.epoch != epoch || !held.live_at(now_ms) {
            return Err(HostingError::LeaseLost);
        }
        held.expires_at_ms = now_ms.saturating_add(lease_ms);
        Ok(held.clone())
    }

    /// Gives up a claim.
    ///
    /// Releasing is **not** shutdown and discharges nothing. The next owner inherits both the
    /// resource and the obligation to stop it.
    ///
    /// # Errors
    ///
    /// Returns [`HostingError::StaleEpoch`] when a newer epoch already owns the deployment and
    /// [`HostingError::LeaseLost`] when this owner holds no claim.
    pub fn release(
        &self,
        owner: &Identifier,
        deployment: &Identifier,
        epoch: u64,
    ) -> Result<(), HostingError> {
        let mut entries = self.entries();
        let entry = entries.get_mut(deployment).ok_or(HostingError::LeaseLost)?;
        if entry.epoch > epoch {
            return Err(HostingError::StaleEpoch);
        }
        match &entry.held {
            Some(held) if &held.owner == owner && held.epoch == epoch => {
                entry.held = None;
                Ok(())
            }
            _ => Err(HostingError::LeaseLost),
        }
    }

    /// The claim currently recorded for a deployment, live or expired.
    pub fn current(&self, deployment: &Identifier) -> Option<Lease> {
        self.entries()
            .get(deployment)
            .and_then(|entry| entry.held.clone())
    }

    /// The highest epoch ever granted for a deployment.
    pub fn epoch(&self, deployment: &Identifier) -> u64 {
        self.entries()
            .get(deployment)
            .map_or(0, |entry| entry.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::LeaseRegistry;
    use crate::{Identifier, error::HostingError};

    fn id(value: &str) -> Identifier {
        Identifier::new(value).expect("identifier")
    }

    #[test]
    fn a_second_owner_is_refused_while_the_claim_is_live() {
        let registry = LeaseRegistry::default();
        let first = registry
            .acquire(&id("a"), &id("d1"), 0, 100)
            .expect("first owner");
        assert_eq!(first.epoch, 1);
        assert_eq!(
            registry.acquire(&id("b"), &id("d1"), 50, 100),
            Err(HostingError::LeaseHeld)
        );
    }

    #[test]
    fn an_epoch_never_repeats_even_after_release() {
        let registry = LeaseRegistry::default();
        let first = registry.acquire(&id("a"), &id("d1"), 0, 100).expect("a");
        registry
            .release(&id("a"), &id("d1"), first.epoch)
            .expect("release");
        assert!(registry.current(&id("d1")).is_none());
        let second = registry.acquire(&id("b"), &id("d1"), 1, 100).expect("b");
        assert_eq!(second.epoch, 2);
        assert_eq!(
            registry.release(&id("a"), &id("d1"), first.epoch),
            Err(HostingError::StaleEpoch)
        );
    }

    #[test]
    fn a_restarted_owner_fences_its_own_previous_incarnation() {
        let registry = LeaseRegistry::default();
        let before = registry.acquire(&id("a"), &id("d1"), 0, 100).expect("a");
        let after = registry.acquire(&id("a"), &id("d1"), 1, 100).expect("a");
        assert!(after.epoch > before.epoch);
        assert_eq!(
            registry.renew(&id("a"), &id("d1"), before.epoch, 2, 100),
            Err(HostingError::StaleEpoch)
        );
    }

    /// The invariant `Controller::fence` relies on instead of comparing the owner and the epoch
    /// of the registry's current lease against its own recorded one.
    ///
    /// Those two comparisons were written in `fence` and could not be tripped by anything: a
    /// held lease is stored together with the epoch it was granted at, and one epoch is granted
    /// to one owner, so at an equal epoch the registry's lease *is* ours. Deleting an
    /// untrippable guard only pays if the thing that makes it untrippable is itself checked,
    /// which is this: across acquisition, renewal, release, expiry and takeover, a held lease
    /// always carries the registry's current epoch and the owner that epoch was granted to.
    #[test]
    fn a_held_lease_always_carries_the_registrys_current_epoch_and_its_grantee() {
        let registry = LeaseRegistry::default();
        let deployment = id("d1");
        let mut expected_owner = id("a");
        registry
            .acquire(&expected_owner, &deployment, 0, 100)
            .expect("first owner");
        let check = |registry: &LeaseRegistry, owner: &Identifier, step: &str| {
            let held = registry.current(&deployment).expect(step);
            assert_eq!(held.epoch, registry.epoch(&deployment), "{step}: epoch");
            assert_eq!(&held.owner, owner, "{step}: owner");
        };
        check(&registry, &expected_owner, "after acquire");
        registry
            .renew(&expected_owner, &deployment, 1, 10, 100)
            .expect("renew");
        check(&registry, &expected_owner, "after renew");
        // The same owner reacquiring: a restarted controller is a new incarnation.
        registry
            .acquire(&expected_owner, &deployment, 20, 100)
            .expect("reacquire");
        check(&registry, &expected_owner, "after reacquire");
        // Expired, then taken over by somebody else.
        expected_owner = id("b");
        registry
            .acquire(&expected_owner, &deployment, 1_000, 100)
            .expect("takeover");
        check(&registry, &expected_owner, "after takeover");
        // An expired claim is still the registry's current one, at the registry's epoch.
        check(&registry, &expected_owner, "after expiry");
        // Released: there is no current lease at all, which `fence` reads as `lease-lost`.
        let epoch = registry.epoch(&deployment);
        registry
            .release(&expected_owner, &deployment, epoch)
            .expect("release");
        assert!(registry.current(&deployment).is_none());
        assert_eq!(
            registry.epoch(&deployment),
            epoch,
            "releasing never lowers the granted epoch"
        );
    }

    #[test]
    fn an_expired_claim_can_be_taken_over() {
        let registry = LeaseRegistry::default();
        registry.acquire(&id("a"), &id("d1"), 0, 100).expect("a");
        assert_eq!(
            registry.acquire(&id("b"), &id("d1"), 100, 100),
            Err(HostingError::LeaseHeld),
            "expiry is inclusive"
        );
        let taken = registry.acquire(&id("b"), &id("d1"), 101, 100).expect("b");
        assert_eq!(taken.epoch, 2);
    }
}
