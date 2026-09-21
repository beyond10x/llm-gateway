//! Adversarial pass 2 over the owned-resource hosting contract.
//!
//! Nothing here opens a socket, reads a credential or allocates a cloud resource.
//!
//! The pass-1 blocker was "a foreign owner tag that is not strictly newer is silently ignored,
//! and the next stop destroys the other owner's compute". The correction routes that tag through
//! `require_stop(StopReason::OwnershipLost)` and keys `Stop`'s refusal on
//! `record.stop_reason == Some(StopReason::OwnershipLost)`.
//!
//! `require_stop` is documented as "Never overwrites the reason that opened it", so the
//! ownership-lost fact is only recorded when no other reason got there first. Every case the
//! unit wrote for a disputed resource reaches the retag from `Active` with `stop_reason: None`.
//! These cases reach it with the slot already occupied, which is what an operator-requested stop
//! and an expired lease both do.

use std::sync::Arc;

use llm_provision::{
    ComputeAuthorization, Controller, DeploymentSpec, FakeProvider, HostingCommand, HostingError,
    HostingPolicy, HostingProvider, Identifier, LeaseRegistry, Phase, StopReason,
};

fn id(value: &str) -> Identifier {
    Identifier::new(value).expect("test identifier")
}

fn policy() -> HostingPolicy {
    HostingPolicy {
        controller: id("controller-a"),
        provider: id("fake"),
        account: id("account-1"),
        ledger: id("scope"),
        max_active: 2,
        max_lifetime_ms: 10_000,
        lease_ms: 1_000,
    }
}

fn spec(name: &str) -> DeploymentSpec {
    DeploymentSpec {
        id: id(name),
        provider: id("fake"),
        account: id("account-1"),
        model: id("requested-model"),
        image: id("requested-image"),
        resource_name: id("pool-slot"),
        requested_lifetime_ms: 5_000,
    }
}

fn authorization() -> ComputeAuthorization {
    ComputeAuthorization {
        ledger: id("scope"),
        reservation: id("reservation-a"),
    }
}

fn open() -> Controller {
    Controller::new(policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy")
}

/// Declare, acquire and provision one deployment, leaving it in `Requested`.
fn provisioned(controller: &mut Controller, fake: &mut FakeProvider, name: &str) {
    controller
        .apply(
            1,
            fake,
            HostingCommand::Declare {
                spec: spec(name),
                request_id: id(&format!("request-{name}")),
                authorization: authorization(),
            },
        )
        .expect("declare");
    controller
        .apply(
            1,
            fake,
            HostingCommand::Acquire {
                deployment: id(name),
            },
        )
        .expect("acquire");
    controller
        .apply(
            1,
            fake,
            HostingCommand::Provision {
                deployment: id(name),
            },
        )
        .expect("provision");
}

fn state(controller: &Controller, name: &str) -> llm_provision::DeploymentRecord {
    let wanted = id(name);
    controller
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment == wanted)
        .expect("declared deployment")
}

/// Whether the provider still holds a resource under this name.
fn provider_still_holds(fake: &mut FakeProvider, name: &Identifier) -> bool {
    fake.inventory()
        .resources
        .iter()
        .any(|resource| &resource.key.name == name)
}

// ---------------------------------------------------------------------------------------------
// An obligation that was already open hides the ownership-lost fact behind it.
// ---------------------------------------------------------------------------------------------

/// `docs/hosting.md:107-110`: "A foreign owner tag that is not a strictly newer epoch now calls
/// `require_stop` with `ownership-lost`, and `Stop` refuses such a record outright with
/// `foreign-resource`."
///
/// Here the operator asked for the stop first, which fills `stop_reason` with
/// `operator-request`. `require_stop` does not overwrite it, so the ownership-lost fact is never
/// recorded, `Stop`'s guard — which is keyed on that reason and on nothing else — does not fire,
/// and the mutation is addressed to a resource the provider says belongs to `controller-b`.
#[test]
fn an_operator_requested_stop_must_not_hide_a_later_foreign_owner_tag() {
    let mut controller = open();
    let mut fake = FakeProvider::new(id("fake"), id("account-1"));
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("first observation");
    assert_eq!(state(&controller, "d1").phase, Phase::Active);

    // An ordinary operator stop. Nothing has been submitted yet; the obligation is merely open.
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::RequestStop {
                deployment: id("d1"),
            },
        )
        .expect("request stop");
    assert_eq!(state(&controller, "d1").phase, Phase::StopRequired);

    // Same epoch, different owner: not a takeover, and not something we may touch either.
    fake.retag(&id("pool-slot"), &id("controller-b"), 1);
    controller
        .apply(4, &mut fake, HostingCommand::Observe)
        .expect("second observation");

    let outcome = controller.apply(
        5,
        &mut fake,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert_eq!(
        fake.stops(),
        0,
        "a stop mutation was addressed to a resource the provider says controller-b owns"
    );
    assert!(
        provider_still_holds(&mut fake, &id("pool-slot")),
        "controller-b's resource was destroyed"
    );
    assert_eq!(
        outcome,
        Err(HostingError::ForeignResource),
        "Stop must refuse a disputed resource whatever else is already owed"
    );
    assert_eq!(
        state(&controller, "d1").stop_reason,
        Some(StopReason::OwnershipLost),
        "the ownership-lost fact must be recorded, not dropped because a reason was already set"
    );
}

/// The same defect reached without an operator: the ownership lease simply ran out first.
///
/// `elapse` fills `stop_reason` with `lease-expired`, the controller reacquires at a new epoch
/// as the contract requires, and the foreign owner tag that follows is then invisible to
/// `Stop`'s guard.
#[test]
fn an_expired_lease_must_not_hide_a_later_foreign_owner_tag() {
    let mut controller = open();
    let mut fake = FakeProvider::new(id("fake"), id("account-1"));
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("first observation");

    // The lease was granted at 1 for 1_000ms, so it is expired at 1_002.
    controller
        .apply(1_002, &mut fake, HostingCommand::Tick)
        .expect("tick past the lease");
    assert_eq!(
        state(&controller, "d1").stop_reason,
        Some(StopReason::LeaseExpired)
    );
    controller
        .apply(
            1_003,
            &mut fake,
            HostingCommand::Acquire {
                deployment: id("d1"),
            },
        )
        .expect("reacquire at a new epoch");

    fake.retag(&id("pool-slot"), &id("controller-b"), 1);
    controller
        .apply(1_004, &mut fake, HostingCommand::Observe)
        .expect("observation");

    let outcome = controller.apply(
        1_005,
        &mut fake,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert_eq!(
        fake.stops(),
        0,
        "a stop mutation was addressed to a resource the provider says controller-b owns"
    );
    assert!(
        provider_still_holds(&mut fake, &id("pool-slot")),
        "controller-b's resource was destroyed"
    );
    assert_eq!(outcome, Err(HostingError::ForeignResource));
}

/// What the two cases above cost, stated as the fact an operator would have to pay for.
///
/// Same sequence as `an_operator_requested_stop_must_not_hide_a_later_foreign_owner_tag`; this
/// one measures the provider's own inventory afterwards rather than the controller's refusal,
/// so the finding does not rest on the call count alone.
#[test]
fn stopping_after_a_hidden_foreign_owner_tag_destroys_the_other_owners_compute() {
    let mut controller = open();
    let mut fake = FakeProvider::new(id("fake"), id("account-1"));
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("first observation");
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::RequestStop {
                deployment: id("d1"),
            },
        )
        .expect("request stop");
    fake.retag(&id("pool-slot"), &id("controller-b"), 1);
    controller
        .apply(4, &mut fake, HostingCommand::Observe)
        .expect("second observation");
    assert!(
        provider_still_holds(&mut fake, &id("pool-slot")),
        "the fixture must start with controller-b's resource present"
    );

    let _ignored = controller.apply(
        5,
        &mut fake,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert!(
        provider_still_holds(&mut fake, &id("pool-slot")),
        "controller-b's resource is gone: this controller destroyed compute it does not own"
    );
    assert_eq!(
        state(&controller, "d1").phase,
        Phase::StopRequired,
        "and the record reports the destruction as a discharged obligation of its own"
    );
}

// ---------------------------------------------------------------------------------------------
// A phase change the controller asks for that the enumerated table does not hold.
// ---------------------------------------------------------------------------------------------

/// `machine.rs` publishes `INTENTS`, "every phase change the controller asks for", and
/// `docs/hosting.md:103-105` says a phase write guarded on `may_become` and nothing else is
/// silent when the table forbids it.
///
/// `RequestStop` asks for exactly one phase change — `require_stop`, which is guarded on
/// `may_become(StopRequired)` and nothing else — and from `Declared` that pair is not an edge.
/// `INTENTS` has no `Declared -> StopRequired` row, so the one command that still reports a
/// forbidden change as success is the one the enumeration missed. `Cancel` refuses the same
/// class with `wrong-phase`.
#[test]
fn a_stop_requested_on_a_declared_deployment_must_not_be_reported_as_done() {
    assert!(
        !Phase::Declared.may_become(Phase::StopRequired),
        "the table forbids the only change RequestStop makes from Declared"
    );
    let mut controller = open();
    let mut fake = FakeProvider::new(id("fake"), id("account-1"));
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-d1"),
                authorization: authorization(),
            },
        )
        .expect("declare");

    let outcome = controller.apply(
        2,
        &mut fake,
        HostingCommand::RequestStop {
            deployment: id("d1"),
        },
    );
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Declared);
    assert_eq!(record.stop_reason, None);
    assert_eq!(
        outcome.err(),
        Some(HostingError::WrongPhase),
        "RequestStop reported success while recording no obligation at all"
    );
}
