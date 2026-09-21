//! Behaviour of the owned-resource hosting contract against the in-process fake provider.
//!
//! Nothing here opens a socket, reads a credential or allocates a cloud resource.

use std::sync::{Arc, Barrier};

use llm_provision::{
    AmbiguousCreate, Completeness, ComputeAuthorization, Controller, DeploymentSpec, Dispatch,
    FakeProvider, HostingCommand, HostingError, HostingPolicy, HostingProvider, Idempotency,
    Identifier, LeaseRegistry, Phase, ProviderState, StopReason,
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

fn provider() -> FakeProvider {
    FakeProvider::new(id("fake"), id("account-1"))
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

fn open() -> Controller {
    Controller::new(policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy")
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

#[test]
fn validating_and_listing_allocate_nothing() {
    let mut controller = open();
    let mut fake = provider();
    controller
        .apply(1, &mut fake, HostingCommand::Validate { spec: spec("d1") })
        .expect("valid specification");
    controller
        .apply(1, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    assert_eq!(fake.allocations(), 0);
    assert_eq!(fake.submits(), 0);
    assert_eq!(fake.stops(), 0);
    assert_eq!(fake.lists(), 1);
    assert!(controller.view().deployments.is_empty());
}

#[test]
fn validation_refuses_a_lifetime_above_the_ceiling_without_calling_the_provider() {
    let mut controller = open();
    let mut fake = provider();
    let mut over = spec("d1");
    over.requested_lifetime_ms = 10_001;
    assert_eq!(
        controller.apply(1, &mut fake, HostingCommand::Validate { spec: over }),
        Err(HostingError::InvalidSpec)
    );
    assert_eq!(fake.submits(), 0);
    assert_eq!(fake.allocations(), 0);
}

#[test]
fn an_accepted_create_records_the_provider_assigned_identity() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Requested);
    assert_eq!(fake.allocations(), 1);
    let observed = record.observed.expect("create answered with a resource");
    assert_eq!(observed.key.name, id("pool-slot"));
    assert_eq!(observed.key.incarnation, id("fake-incarnation-1"));
}

#[test]
fn an_observed_deployment_never_echoes_the_requested_specification() {
    let mut controller = open();
    let mut fake = provider();
    // A provider that reports neither the served model nor an endpoint.
    fake.report_served_model(false);
    fake.report_endpoint(false);
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.requested.model, id("requested-model"));
    let observed = record.observed.expect("observed");
    assert_eq!(observed.served_model, None);
    assert_eq!(observed.endpoint, None);
}

#[test]
fn unreported_readiness_is_unknown_and_never_ready() {
    let mut controller = open();
    let mut fake = provider();
    fake.report_readiness(None);
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Active);
    assert_eq!(record.observed.expect("observed").ready, None);

    fake.report_readiness(Some(false));
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    assert_eq!(
        state(&controller, "d1").observed.expect("observed").ready,
        Some(false)
    );

    fake.report_readiness(Some(true));
    controller
        .apply(4, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let ready = state(&controller, "d1").observed.expect("observed");
    assert_eq!(ready.ready, Some(true));
    assert_eq!(ready.state, Some(ProviderState::Running));
}

#[test]
fn concurrent_acquisition_grants_exactly_one_lease() {
    let registry = Arc::new(LeaseRegistry::default());
    let deployment = id("d1");
    let barrier = Arc::new(Barrier::new(8));
    let granted = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                let deployment = deployment.clone();
                let owner = id(&format!("controller-{index}"));
                scope.spawn(move || {
                    barrier.wait();
                    registry.acquire(&owner, &deployment, 1, 1_000)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("acquire thread"))
            .collect::<Vec<_>>()
    });
    let leases: Vec<_> = granted
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .collect();
    assert_eq!(
        leases.len(),
        1,
        "exactly one controller may own the resource"
    );
    assert_eq!(leases[0].epoch, 1);
    // `all` over a collection the fixture leaves empty asserts nothing, so the size is pinned
    // beside it: seven of the eight racers must have been refused, every one with `lease-held`.
    let refused: Vec<_> = granted
        .iter()
        .filter_map(|result| result.as_ref().err())
        .collect();
    assert_eq!(refused.len(), 7, "seven racers must have been refused");
    assert!(
        refused
            .iter()
            .all(|error| **error == HostingError::LeaseHeld)
    );
}

#[test]
fn a_stale_epoch_is_refused_before_the_provider_is_called() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut first = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut first, &mut fake, "d1");

    let mut second_policy = policy();
    second_policy.controller = id("controller-b");
    let mut second = Controller::new(second_policy, Arc::clone(&registry), 0).expect("policy");
    second
        .apply(
            2_000,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-2"),
                authorization: authorization(),
            },
        )
        .expect("declare");
    // The first lease has expired, so a second controller takes it at a higher epoch.
    second
        .apply(
            2_000,
            &mut fake,
            HostingCommand::Acquire {
                deployment: id("d1"),
            },
        )
        .expect("acquire at a higher epoch");

    let stops_before = fake.stops();
    assert_eq!(
        first.apply(
            2_000,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1")
            }
        ),
        Err(HostingError::StaleEpoch)
    );
    assert_eq!(
        fake.stops(),
        stops_before,
        "a fenced controller mutates nothing"
    );
}

#[test]
fn an_expired_lease_is_not_evidence_that_the_resource_stopped() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    controller
        .apply(5_000, &mut fake, HostingCommand::Tick)
        .expect("tick");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::StopRequired);
    assert_eq!(record.stop_reason, Some(StopReason::LeaseExpired));
    assert_eq!(record.stop_evidence, None);
    assert_eq!(controller.view().totals.stop_required, vec![id("d1")]);
}

#[test]
fn releasing_a_lease_is_not_shutdown() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(
            2,
            &mut fake,
            HostingCommand::Release {
                deployment: id("d1"),
            },
        )
        .expect("release");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Requested);
    assert_eq!(record.stop_evidence, None);
    assert_eq!(fake.stops(), 0);
}

#[test]
fn disconnecting_a_client_is_not_shutdown() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::Connect {
                deployment: id("d1"),
            },
        )
        .expect("connect");
    let attached = state(&controller, "d1");
    assert!(attached.connected);
    assert_eq!(attached.phase, Phase::Active);
    controller
        .apply(
            4,
            &mut fake,
            HostingCommand::Disconnect {
                deployment: id("d1"),
            },
        )
        .expect("disconnect");
    let record = state(&controller, "d1");
    assert!(!record.connected);
    assert_eq!(record.phase, Phase::Active);
    assert_eq!(record.stop_evidence, None);
    assert_eq!(fake.stops(), 0);
}

#[test]
fn an_incomplete_inventory_is_not_evidence_of_absence() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    fake.inventory_completeness(Completeness::Partial);
    fake.vanish(&id("pool-slot"));
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Active);
    assert_eq!(record.stop_evidence, None);
}

#[test]
fn absence_from_a_complete_inventory_discharges_the_obligation() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    fake.vanish(&id("pool-slot"));
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Stopped);
    assert_eq!(record.stop_evidence, Some(id("inventory-complete-absent")));
    assert!(controller.view().totals.stop_required.is_empty());
}

#[test]
fn a_reused_name_with_a_new_incarnation_is_not_our_resource() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    fake.vanish(&id("pool-slot"));
    // The provider hands the same name to somebody else's resource.
    fake.reuse_name(&id("pool-slot"), &id("controller-z"));
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(
        record.phase,
        Phase::Stopped,
        "a same-named later incarnation is not the resource we owned"
    );
    assert_eq!(
        record.observed.expect("observed").key.incarnation,
        id("fake-incarnation-1")
    );
    assert_eq!(fake.stops(), 0, "we never stop somebody else's resource");
}

#[test]
fn an_ambiguous_create_keeps_an_open_obligation_and_never_retries_blindly() {
    let mut controller = open();
    let mut fake = provider();
    fake.idempotency(Idempotency::Unsupported);
    fake.next_create(Dispatch::Unknown);
    fake.ambiguous_create(AmbiguousCreate::Allocates);
    provisioned(&mut controller, &mut fake, "d1");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Uncertain);
    assert_eq!(record.observed, None);
    assert_eq!(record.stop_reason, Some(StopReason::AmbiguousMutation));
    assert_eq!(controller.view().totals.stop_required, vec![id("d1")]);

    let submits = fake.submits();
    assert_eq!(
        controller.apply(
            2,
            &mut fake,
            HostingCommand::Retry {
                deployment: id("d1")
            }
        ),
        Err(HostingError::IdempotencyUnsupported)
    );
    assert_eq!(fake.submits(), submits, "no blind second create");
}

#[test]
fn an_ambiguous_create_is_resolved_by_adopting_the_resource_it_left_behind() {
    let mut controller = open();
    let mut fake = provider();
    fake.next_create(Dispatch::Unknown);
    fake.ambiguous_create(AmbiguousCreate::Allocates);
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Active);
    assert_eq!(
        record.stop_reason, None,
        "adoption resolves the ambiguity that opened the obligation"
    );
    assert_eq!(
        record.observed.expect("adopted").key.incarnation,
        id("fake-incarnation-1")
    );
}

#[test]
fn an_ambiguous_create_that_allocated_nothing_settles_against_a_complete_inventory() {
    let mut controller = open();
    let mut fake = provider();
    fake.next_create(Dispatch::Unknown);
    fake.ambiguous_create(AmbiguousCreate::DoesNotAllocate);
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Stopped);
    assert_eq!(record.stop_evidence, Some(id("inventory-complete-absent")));
}

#[test]
fn a_provider_that_honours_the_request_id_may_be_retried_without_allocating_twice() {
    let mut controller = open();
    let mut fake = provider();
    fake.next_create(Dispatch::Unknown);
    fake.ambiguous_create(AmbiguousCreate::Allocates);
    provisioned(&mut controller, &mut fake, "d1");
    assert_eq!(fake.allocations(), 1);
    controller
        .apply(
            2,
            &mut fake,
            HostingCommand::Retry {
                deployment: id("d1"),
            },
        )
        .expect("idempotent retry");
    assert_eq!(fake.submits(), 2);
    assert_eq!(
        fake.allocations(),
        1,
        "the request id is the idempotency key"
    );
    assert_eq!(state(&controller, "d1").phase, Phase::Requested);
}

#[test]
fn a_restart_keeps_identity_and_obligations_but_forgets_liveness() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    assert_eq!(
        state(&controller, "d1").observed.expect("observed").ready,
        Some(true)
    );

    let snapshot = controller.snapshot();
    drop(controller);
    let restored =
        Controller::restore(policy(), Arc::clone(&registry), snapshot, 3).expect("restore");
    let record = state(&restored, "d1");
    assert_eq!(record.phase, Phase::Active);
    let observed = record.observed.expect("identity survives restart");
    assert_eq!(observed.key.incarnation, id("fake-incarnation-1"));
    assert_eq!(observed.ready, None, "a stale readiness is not a readiness");
    assert_eq!(observed.endpoint, None);
    assert_eq!(observed.state, None);
}

#[test]
fn a_restarted_controller_must_take_a_new_lease_before_it_mutates() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    let snapshot = controller.snapshot();
    drop(controller);
    let mut restored =
        Controller::restore(policy(), Arc::clone(&registry), snapshot, 3).expect("restore");
    assert_eq!(
        restored.apply(
            3,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1")
            }
        ),
        Err(HostingError::LeaseLost)
    );
    assert_eq!(fake.stops(), 0);
    restored
        .apply(
            3,
            &mut fake,
            HostingCommand::Acquire {
                deployment: id("d1"),
            },
        )
        .expect("reacquire");
    restored
        .apply(
            3,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1"),
            },
        )
        .expect("stop");
    assert_eq!(fake.stops(), 1);
    assert_eq!(state(&restored, "d1").phase, Phase::Stopped);
}

#[test]
fn cancellation_only_releases_a_deployment_that_was_never_provisioned() {
    let mut controller = open();
    let mut fake = provider();
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-1"),
                authorization: authorization(),
            },
        )
        .expect("declare");
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Cancel {
                deployment: id("d1"),
            },
        )
        .expect("cancel an unsent deployment");
    assert_eq!(state(&controller, "d1").phase, Phase::Cancelled);

    let mut second = open();
    let mut fake = provider();
    provisioned(&mut second, &mut fake, "d2");
    assert_eq!(
        second.apply(
            2,
            &mut fake,
            HostingCommand::Cancel {
                deployment: id("d2")
            }
        ),
        Err(HostingError::WrongPhase)
    );
    assert_eq!(state(&second, "d2").phase, Phase::Requested);
}

#[test]
fn the_resource_ceiling_refuses_before_anything_is_allocated() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    provisioned(&mut controller, &mut fake, "d2");
    assert_eq!(fake.allocations(), 2);
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d3"),
                request_id: id("request-3"),
                authorization: authorization(),
            },
        )
        .expect("declare");
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Acquire {
                deployment: id("d3"),
            },
        )
        .expect("acquire");
    assert_eq!(
        controller.apply(
            1,
            &mut fake,
            HostingCommand::Provision {
                deployment: id("d3")
            }
        ),
        Err(HostingError::CapacityExceeded)
    );
    assert_eq!(fake.allocations(), 2);
    assert_eq!(fake.submits(), 2);
    assert!(controller.view().totals.at_capacity);
}

#[test]
fn the_time_ceiling_requires_a_stop_rather_than_declaring_one() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    controller
        .apply(
            2,
            &mut fake,
            HostingCommand::Renew {
                deployment: id("d1"),
            },
        )
        .expect("renew");
    controller
        .apply(5_002, &mut fake, HostingCommand::Tick)
        .expect("tick past the requested lifetime");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::StopRequired);
    assert_eq!(record.stop_reason, Some(StopReason::LifetimeExceeded));
    assert_eq!(record.stop_evidence, None);
    assert_eq!(fake.stops(), 0, "the ceiling obliges, it does not act");
}

#[test]
fn provisioning_without_a_current_lease_is_refused() {
    let mut controller = open();
    let mut fake = provider();
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-1"),
                authorization: authorization(),
            },
        )
        .expect("declare");
    assert_eq!(
        controller.apply(
            1,
            &mut fake,
            HostingCommand::Provision {
                deployment: id("d1")
            }
        ),
        Err(HostingError::LeaseLost)
    );
    assert_eq!(fake.submits(), 0);
}

#[test]
fn a_refused_stop_keeps_the_obligation_open() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    fake.next_stop(Dispatch::Unknown);
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1"),
            },
        )
        .expect("ambiguous stop is recorded, not refused");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::StopRequired);
    assert_eq!(record.stop_evidence, None);
    assert_eq!(controller.view().totals.stop_required, vec![id("d1")]);
}

#[test]
fn an_explicit_confirmation_carries_its_own_evidence() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(
            2,
            &mut fake,
            HostingCommand::RequestStop {
                deployment: id("d1"),
            },
        )
        .expect("request stop");
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::ConfirmStopped {
                deployment: id("d1"),
                evidence: id("invoice-line-7"),
            },
        )
        .expect("confirm");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Stopped);
    assert_eq!(record.stop_evidence, Some(id("invoice-line-7")));
    assert!(controller.view().totals.stop_required.is_empty());
}

#[test]
fn a_higher_epoch_owner_disowns_our_record_and_we_stop_mutating_it() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    // Another controller takes the lease after ours expires and retags the resource.
    registry
        .acquire(&id("controller-b"), &id("d1"), 2_000, 1_000)
        .expect("second owner");
    fake.retag(&id("pool-slot"), &id("controller-b"), 2);
    controller
        .apply(2_001, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Disowned);
    assert!(controller.view().totals.stop_required.is_empty());
    assert_eq!(
        controller.apply(
            2_002,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1")
            }
        ),
        Err(HostingError::StaleEpoch)
    );
    assert_eq!(fake.stops(), 0);
}

#[test]
fn the_clock_cannot_move_backwards() {
    let mut controller = open();
    let mut fake = provider();
    controller
        .apply(100, &mut fake, HostingCommand::Tick)
        .expect("tick");
    assert_eq!(
        controller.apply(99, &mut fake, HostingCommand::Tick),
        Err(HostingError::ClockReversed)
    );
    assert_eq!(controller.view().now_ms, 100);
}

#[test]
fn a_duplicate_declaration_is_not_new_authority() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    assert_eq!(
        controller.apply(
            2,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-9"),
                authorization: authorization(),
            }
        ),
        Err(HostingError::Duplicate)
    );
    assert_eq!(state(&controller, "d1").request_id, id("request-d1"));
}

#[test]
fn a_specification_for_another_account_is_refused() {
    let mut controller = open();
    let mut fake = provider();
    let mut foreign = spec("d1");
    foreign.account = id("account-2");
    assert_eq!(
        controller.apply(1, &mut fake, HostingCommand::Validate { spec: foreign }),
        Err(HostingError::InvalidSpec)
    );
}

#[test]
fn every_unmapped_transition_is_refused() {
    // The resolved lifecycle: no pair outside this table is permitted.
    let permitted: &[(Phase, Phase)] = &[
        (Phase::Declared, Phase::Requested),
        (Phase::Declared, Phase::Uncertain),
        (Phase::Declared, Phase::Cancelled),
        (Phase::Requested, Phase::Requested),
        (Phase::Requested, Phase::Active),
        (Phase::Requested, Phase::StopRequired),
        (Phase::Requested, Phase::Stopped),
        (Phase::Requested, Phase::Disowned),
        (Phase::Active, Phase::Active),
        (Phase::Active, Phase::StopRequired),
        (Phase::Active, Phase::Stopped),
        (Phase::Active, Phase::Disowned),
        (Phase::Uncertain, Phase::Uncertain),
        (Phase::Uncertain, Phase::Requested),
        (Phase::Uncertain, Phase::Active),
        (Phase::Uncertain, Phase::StopRequired),
        (Phase::Uncertain, Phase::Stopped),
        (Phase::Uncertain, Phase::Disowned),
        (Phase::StopRequired, Phase::StopRequired),
        (Phase::StopRequired, Phase::Stopped),
        (Phase::StopRequired, Phase::Disowned),
    ];
    for &from in Phase::ALL {
        for &to in Phase::ALL {
            let expected = permitted.contains(&(from, to));
            assert_eq!(
                from.may_become(to),
                expected,
                "transition {from:?} -> {to:?}"
            );
        }
    }
    assert!(!Phase::Stopped.may_become(Phase::Active));
    assert!(!Phase::StopRequired.may_become(Phase::Cancelled));
    assert!(!Phase::Uncertain.may_become(Phase::Cancelled));
    assert!(Phase::Stopped.terminal());
    assert!(Phase::Cancelled.terminal());
    assert!(Phase::Disowned.terminal());
}

#[test]
fn an_invalid_policy_is_refused_before_a_controller_exists() {
    let mut zero = policy();
    zero.max_active = 0;
    assert_eq!(
        Controller::new(zero, Arc::new(LeaseRegistry::default()), 0).err(),
        Some(HostingError::InvalidPolicy)
    );
    let mut forever = policy();
    forever.max_lifetime_ms = 0;
    assert_eq!(
        Controller::new(forever, Arc::new(LeaseRegistry::default()), 0).err(),
        Some(HostingError::InvalidPolicy)
    );
}

#[test]
fn the_fake_provider_reports_its_own_identity_and_never_allocates_while_listing() {
    let mut fake = provider();
    let inventory = fake.inventory();
    assert!(inventory.complete());
    assert!(inventory.resources.is_empty());
    assert_eq!(fake.allocations(), 0);
    assert!(fake.honours_idempotency_key());
}

#[test]
fn an_authorization_from_another_ledger_is_refused() {
    let mut controller = open();
    let mut fake = provider();
    assert_eq!(
        controller.apply(
            1,
            &mut fake,
            HostingCommand::Declare {
                spec: spec("d1"),
                request_id: id("request-d1"),
                authorization: ComputeAuthorization {
                    ledger: id("other-scope"),
                    reservation: id("reservation-a"),
                },
            }
        ),
        Err(HostingError::Unauthorized)
    );
    assert!(controller.view().deployments.is_empty());
    assert_eq!(fake.submits(), 0);
}

#[test]
fn a_provider_reported_termination_is_evidence_that_the_resource_stopped() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    fake.terminate(&id("pool-slot"));
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Stopped);
    assert_eq!(record.stop_evidence, Some(id("provider-terminated")));
    assert_eq!(
        record.observed.expect("observed").state,
        Some(ProviderState::Terminated)
    );
}

#[test]
fn the_documented_evidence_constants_are_valid_identifiers() {
    assert_eq!(
        Identifier::new(llm_provision::EVIDENCE_ABSENT_FROM_COMPLETE_INVENTORY),
        Ok(id("inventory-complete-absent"))
    );
    assert_eq!(
        Identifier::new(llm_provision::EVIDENCE_PROVIDER_TERMINATED),
        Ok(id("provider-terminated"))
    );
}

// ---------------------------------------------------------------------------------------------
// Published bounds, each measured at the literal number and one past it.
//
// Every expected value below is a literal. A bound asserted against its own constant — the
// `CONSTANT + 1` shape — cannot detect the constant moving, which is exactly how five defects
// in the ceilings survived a suite of fifty cases: every fixture used the same ceiling and the
// same lifetime pair, so no fixture was ever *at* a boundary.
// ---------------------------------------------------------------------------------------------

#[test]
fn the_published_resource_ceiling_is_four_thousand_and_ninety_six() {
    assert_eq!(llm_provision::MAX_ACTIVE_CEILING, 4_096);
    for (max_active, admitted) in [(0, false), (1, true), (4_096, true), (4_097, false)] {
        let mut candidate = policy();
        candidate.max_active = max_active;
        let outcome = Controller::new(candidate, Arc::new(LeaseRegistry::default()), 0);
        assert_eq!(outcome.is_ok(), admitted, "max_active {max_active}");
        if !admitted {
            assert_eq!(outcome.err(), Some(HostingError::InvalidPolicy));
        }
    }
}

#[test]
fn a_zero_time_ceiling_or_lease_is_refused_and_one_millisecond_is_admitted() {
    for (max_lifetime_ms, admitted) in [(0, false), (1, true)] {
        let mut candidate = policy();
        candidate.max_lifetime_ms = max_lifetime_ms;
        assert_eq!(
            Controller::new(candidate, Arc::new(LeaseRegistry::default()), 0).is_ok(),
            admitted,
            "max_lifetime_ms {max_lifetime_ms}"
        );
    }
    for (lease_ms, admitted) in [(0, false), (1, true)] {
        let mut candidate = policy();
        candidate.lease_ms = lease_ms;
        assert_eq!(
            Controller::new(candidate, Arc::new(LeaseRegistry::default()), 0).is_ok(),
            admitted,
            "lease_ms {lease_ms}"
        );
    }
}

#[test]
fn a_requested_lifetime_is_admitted_at_the_ceiling_and_refused_one_past_it() {
    let mut controller = open();
    let mut fake = provider();
    // policy().max_lifetime_ms is 10_000; these are the literals either side of it, and the
    // literals either side of the lower bound.
    for (requested_lifetime_ms, admitted) in
        [(0, false), (1, true), (10_000, true), (10_001, false)]
    {
        let mut candidate = spec("d1");
        candidate.requested_lifetime_ms = requested_lifetime_ms;
        let outcome = controller.apply(1, &mut fake, HostingCommand::Validate { spec: candidate });
        assert_eq!(
            outcome.is_ok(),
            admitted,
            "requested_lifetime_ms {requested_lifetime_ms}"
        );
        if !admitted {
            assert_eq!(outcome.err(), Some(HostingError::InvalidSpec));
        }
    }
    assert_eq!(fake.submits(), 0);
}

#[test]
fn the_last_slot_under_the_resource_ceiling_is_admitted() {
    // The refusal at the ceiling has a case already. This is the other half: one slot below
    // it the provision must go through, and `at_capacity` must still be false. Without it a
    // ceiling that refuses everything, or one multiplied by ten, reads the same.
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    assert!(!controller.view().totals.at_capacity);
    assert_eq!(controller.view().totals.active_count, 1);
    provisioned(&mut controller, &mut fake, "d2");
    assert!(controller.view().totals.at_capacity);
    assert_eq!(fake.allocations(), 2);
}

/// A policy with a lease long enough that lease expiry never masks the time ceiling.
fn patient_policy() -> HostingPolicy {
    HostingPolicy {
        lease_ms: 1_000_000,
        ..policy()
    }
}

#[test]
fn the_time_ceiling_does_not_fire_at_its_deadline_and_does_fire_one_millisecond_later() {
    let mut controller =
        Controller::new(patient_policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    // Provisioned at t=1 with a requested lifetime of 5_000, so the deadline is 5_001.
    controller
        .apply(5_001, &mut fake, HostingCommand::Tick)
        .expect("tick at the deadline");
    let at_the_deadline = state(&controller, "d1");
    assert_eq!(
        at_the_deadline.phase,
        Phase::Requested,
        "the deadline instant is still inside the lifetime"
    );
    assert_eq!(at_the_deadline.stop_reason, None);

    controller
        .apply(5_002, &mut fake, HostingCommand::Tick)
        .expect("tick one past the deadline");
    let past_it = state(&controller, "d1");
    assert_eq!(past_it.phase, Phase::StopRequired);
    assert_eq!(past_it.stop_reason, Some(StopReason::LifetimeExceeded));
}

#[test]
fn a_policy_tightened_across_a_restart_lowers_the_deadline_to_the_tighter_ceiling() {
    // `effective_lifetime_ms` is the minimum of the requested lifetime and the policy
    // ceiling. `Declare` refuses a request above the ceiling, so the two only ever differ
    // after a restart into a tightened policy — which is the one place the minimum is
    // observable, and the reason it must be a minimum rather than a maximum.
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller =
        Controller::new(patient_policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    assert_eq!(
        spec("d1").effective_lifetime_ms(&patient_policy()),
        5_000,
        "the requested lifetime is the lower of the two here"
    );
    let tightened = HostingPolicy {
        max_lifetime_ms: 2_000,
        ..patient_policy()
    };
    assert_eq!(
        spec("d1").effective_lifetime_ms(&tightened),
        2_000,
        "the tightened ceiling is the lower of the two here"
    );

    let snapshot = controller.snapshot();
    let mut restored =
        Controller::restore(tightened, Arc::clone(&registry), snapshot, 3).expect("restore");
    // Started at t=1, so the tightened deadline is 2_001.
    restored
        .apply(2_001, &mut fake, HostingCommand::Tick)
        .expect("tick at the tightened deadline");
    assert_eq!(state(&restored, "d1").stop_reason, None);
    restored
        .apply(2_002, &mut fake, HostingCommand::Tick)
        .expect("tick one past the tightened deadline");
    assert_eq!(
        state(&restored, "d1").stop_reason,
        Some(StopReason::LifetimeExceeded),
        "a restart into a tightened policy must use the tighter ceiling"
    );
}

#[test]
fn an_identifier_is_admitted_at_two_hundred_and_fifty_six_bytes_and_refused_at_two_hundred_and_fifty_seven()
 {
    assert!(Identifier::new("i".repeat(256)).is_ok());
    assert!(Identifier::new("i".repeat(257)).is_err());
    assert!(Identifier::new("i").is_ok());
    assert!(Identifier::new("").is_err());
}

#[test]
fn a_lease_is_live_at_its_expiry_instant_and_expired_one_millisecond_later() {
    let registry = LeaseRegistry::default();
    registry
        .acquire(&id("a"), &id("d1"), 0, 1_000)
        .expect("first owner");
    assert_eq!(
        registry.acquire(&id("b"), &id("d1"), 1_000, 1_000),
        Err(HostingError::LeaseHeld),
        "expiry is inclusive, so the claim is still live at 1000"
    );
    assert!(registry.acquire(&id("b"), &id("d1"), 1_001, 1_000).is_ok());
}

// ---------------------------------------------------------------------------------------------
// Ownership the provider disputes.
// ---------------------------------------------------------------------------------------------

#[test]
fn a_foreign_owner_tag_without_a_newer_epoch_obliges_a_stop_rather_than_nothing() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("first observation");
    assert_eq!(state(&controller, "d1").phase, Phase::Active);

    // Same epoch, different owner: not a takeover, and not something we may touch either.
    fake.retag(&id("pool-slot"), &id("controller-b"), 1);
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("second observation");
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::StopRequired);
    assert_eq!(record.stop_reason, Some(StopReason::OwnershipLost));
    assert_eq!(controller.view().totals.stop_required, vec![id("d1")]);

    assert_eq!(
        controller.apply(
            4,
            &mut fake,
            HostingCommand::Stop {
                deployment: id("d1")
            }
        ),
        Err(HostingError::ForeignResource)
    );
    assert_eq!(
        fake.stops(),
        0,
        "we never stop a resource somebody else owns"
    );
}

#[test]
fn a_disowned_record_reports_its_obligation_as_transferred_rather_than_vanished() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    registry
        .acquire(&id("controller-b"), &id("d1"), 2_000, 1_000)
        .expect("second owner");
    fake.retag(&id("pool-slot"), &id("controller-b"), 2);
    controller
        .apply(2_001, &mut fake, HostingCommand::Observe)
        .expect("inventory");

    let totals = controller.view().totals;
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::Disowned);
    assert_eq!(
        record.stop_evidence, None,
        "nothing stopped, so nothing is evidence that anything stopped"
    );
    assert!(totals.stop_required.is_empty());
    assert_eq!(
        totals.transferred,
        vec![id("d1")],
        "the obligation moved with the ownership; it did not disappear"
    );
}

#[test]
fn a_create_answer_outside_this_scope_is_not_adopted_and_nothing_is_addressed_to_it() {
    let mut controller = open();
    let mut fake = provider();
    // The fake's own account, changed under the controller: every answer it gives now names
    // a resource this scope does not own.
    let mut foreign = FakeProvider::new(id("fake"), id("account-9"));
    foreign.report_readiness(Some(true));
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
    controller
        .apply(
            1,
            &mut fake,
            HostingCommand::Acquire {
                deployment: id("d1"),
            },
        )
        .expect("acquire");
    controller
        .apply(
            1,
            &mut foreign,
            HostingCommand::Provision {
                deployment: id("d1"),
            },
        )
        .expect("the mutation was submitted; only its answer is unaccountable");

    let record = state(&controller, "d1");
    assert_eq!(
        record.observed, None,
        "an answer this scope cannot account for is not an adoption"
    );
    assert_eq!(record.phase, Phase::Uncertain);
    assert_eq!(record.stop_reason, Some(StopReason::OwnershipLost));
    assert_eq!(
        controller.apply(
            2,
            &mut foreign,
            HostingCommand::Stop {
                deployment: id("d1")
            }
        ),
        Err(HostingError::ForeignResource)
    );
    assert_eq!(foreign.stops(), 0);
    assert_eq!(foreign.allocations(), 1, "the resource really was created");
}

#[test]
fn a_restored_record_claiming_another_scopes_resource_is_refused() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    let mut snapshot = controller.snapshot();
    let observed = snapshot[0].observed.as_mut().expect("observed");
    observed.key.account = id("account-9");
    assert_eq!(
        Controller::restore(policy(), Arc::clone(&registry), snapshot, 5).err(),
        Some(HostingError::ForeignResource)
    );
}

#[test]
fn a_restart_cannot_reopen_a_controller_at_an_earlier_instant() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(900, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let snapshot = controller.snapshot();
    // 900 is the latest instant the snapshot witnesses; the instant itself is admitted and
    // one before it is refused, through the same guard `apply` uses.
    assert_eq!(
        Controller::restore(policy(), Arc::clone(&registry), snapshot.clone(), 899).err(),
        Some(HostingError::ClockReversed)
    );
    assert!(Controller::restore(policy(), Arc::clone(&registry), snapshot, 900).is_ok());
    assert!(
        Controller::restore(policy(), Arc::clone(&registry), Vec::new(), 0).is_ok(),
        "a snapshot that witnesses nothing constrains nothing, and claims nothing"
    );
}

#[test]
fn a_complete_listing_is_not_enough_to_settle_an_unresolved_create() {
    // Two providers, identical except that one echoes the idempotency key. Completeness is
    // the same in both; only one of them can answer "was anything created for my request?".
    let mut controller = open();
    let mut fake = provider();
    fake.next_create(Dispatch::Unknown);
    fake.ambiguous_create(AmbiguousCreate::Allocates);
    fake.echo_request_id(false);
    provisioned(&mut controller, &mut fake, "d1");
    assert_eq!(state(&controller, "d1").phase, Phase::Uncertain);
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let silent = state(&controller, "d1");
    assert_eq!(
        silent.phase,
        Phase::Uncertain,
        "a listing that names a resource without an idempotency key cannot rule mine out"
    );
    assert_eq!(silent.stop_evidence, None);

    fake.echo_request_id(true);
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    assert_eq!(
        state(&controller, "d1").phase,
        Phase::Active,
        "once the listing answers, the same resource is adopted"
    );
}

// ---------------------------------------------------------------------------------------------
// Nothing is published that nothing can produce.
// ---------------------------------------------------------------------------------------------

/// One controller, one fake provider and the registry they share.
struct Fixture {
    controller: Controller,
    fake: FakeProvider,
    leases: Arc<LeaseRegistry>,
}

impl Fixture {
    fn open() -> Self {
        let leases = Arc::new(LeaseRegistry::default());
        Self {
            controller: Controller::new(policy(), Arc::clone(&leases), 0).expect("policy"),
            fake: FakeProvider::new(id("fake"), id("account-1")),
            leases,
        }
    }

    fn declare(&mut self, name: &str) {
        self.controller
            .apply(
                1,
                &mut self.fake,
                HostingCommand::Declare {
                    spec: spec(name),
                    request_id: id(&format!("request-{name}")),
                    authorization: authorization(),
                },
            )
            .expect("declare");
    }

    fn bring_up(&mut self, name: &str) {
        provisioned(&mut self.controller, &mut self.fake, name);
    }

    fn refuse(&mut self, at_ms: u64, command: HostingCommand) -> HostingError {
        self.controller
            .apply(at_ms, &mut self.fake, command)
            .expect_err("this command must be refused")
    }

    fn accept(&mut self, at_ms: u64, command: HostingCommand) {
        self.controller
            .apply(at_ms, &mut self.fake, command)
            .expect("this command must be accepted");
    }

    fn stop(&mut self, at_ms: u64) -> HostingError {
        self.refuse(
            at_ms,
            HostingCommand::Stop {
                deployment: id("d1"),
            },
        )
    }

    /// Observe, hand the resource somebody else's owner tag, observe again.
    fn disputed(&mut self) {
        self.accept(2, HostingCommand::Observe);
        self.fake.retag(&id("pool-slot"), &id("controller-b"), 1);
        self.accept(3, HostingCommand::Observe);
    }
}

/// Produces each refusal through a real sequence of public calls.
///
/// The match is exhaustive, so a refusal added to [`HostingError`] without a way to reach it
/// does not compile. That is the check: a published code no call can emit is a promise to a
/// reader of the contract that nothing keeps. The mirror of it — a guard nothing can trip —
/// is why `Stop` carries no scope check on an identity that cannot reach a record.
fn emit(error: HostingError) -> HostingError {
    let fixture = Fixture::open();
    match error {
        HostingError::InvalidPolicy => invalid_policy(fixture),
        HostingError::InvalidSpec => a_spec_for_another_account(fixture),
        HostingError::Unauthorized => an_authorization_from_another_ledger(fixture),
        HostingError::Duplicate => a_second_declaration(fixture),
        HostingError::Missing => a_command_for_no_deployment(fixture),
        HostingError::WrongPhase => cancelling_after_the_create(fixture),
        HostingError::ClockReversed => a_clock_that_went_back(fixture),
        HostingError::CapacityExceeded => one_past_the_resource_ceiling(fixture),
        HostingError::LeaseHeld => a_second_owner(&fixture),
        HostingError::LeaseLost => provisioning_with_no_lease(fixture),
        HostingError::StaleEpoch => mutating_after_a_takeover(fixture),
        HostingError::ForeignResource => stopping_a_disputed_resource(fixture),
        HostingError::AmbiguousMutation => stopping_what_was_never_identified(fixture),
        HostingError::IdempotencyUnsupported => retrying_a_provider_that_cannot(fixture),
    }
}

fn invalid_policy(fixture: Fixture) -> HostingError {
    let mut zero = policy();
    zero.max_active = 0;
    Controller::new(zero, fixture.leases, 0).expect_err("invalid policy")
}

fn a_spec_for_another_account(mut fixture: Fixture) -> HostingError {
    let mut foreign = spec("d1");
    foreign.account = id("account-2");
    fixture.refuse(1, HostingCommand::Validate { spec: foreign })
}

fn an_authorization_from_another_ledger(mut fixture: Fixture) -> HostingError {
    fixture.refuse(
        1,
        HostingCommand::Declare {
            spec: spec("d1"),
            request_id: id("request-d1"),
            authorization: ComputeAuthorization {
                ledger: id("other-scope"),
                reservation: id("reservation-a"),
            },
        },
    )
}

fn a_second_declaration(mut fixture: Fixture) -> HostingError {
    fixture.bring_up("d1");
    fixture.refuse(
        2,
        HostingCommand::Declare {
            spec: spec("d1"),
            request_id: id("request-again"),
            authorization: authorization(),
        },
    )
}

fn a_command_for_no_deployment(mut fixture: Fixture) -> HostingError {
    fixture.refuse(
        1,
        HostingCommand::Acquire {
            deployment: id("never-declared"),
        },
    )
}

fn cancelling_after_the_create(mut fixture: Fixture) -> HostingError {
    fixture.bring_up("d1");
    fixture.refuse(
        2,
        HostingCommand::Cancel {
            deployment: id("d1"),
        },
    )
}

fn a_clock_that_went_back(mut fixture: Fixture) -> HostingError {
    fixture.accept(100, HostingCommand::Tick);
    fixture.refuse(99, HostingCommand::Tick)
}

fn one_past_the_resource_ceiling(mut fixture: Fixture) -> HostingError {
    fixture.bring_up("d1");
    fixture.bring_up("d2");
    fixture.declare("d3");
    fixture.accept(
        1,
        HostingCommand::Acquire {
            deployment: id("d3"),
        },
    );
    fixture.refuse(
        1,
        HostingCommand::Provision {
            deployment: id("d3"),
        },
    )
}

fn a_second_owner(fixture: &Fixture) -> HostingError {
    fixture
        .leases
        .acquire(&id("controller-a"), &id("d1"), 1, 1_000)
        .expect("first owner");
    fixture
        .leases
        .acquire(&id("controller-b"), &id("d1"), 1, 1_000)
        .expect_err("second owner")
}

fn provisioning_with_no_lease(mut fixture: Fixture) -> HostingError {
    fixture.declare("d1");
    fixture.refuse(
        1,
        HostingCommand::Provision {
            deployment: id("d1"),
        },
    )
}

fn mutating_after_a_takeover(mut fixture: Fixture) -> HostingError {
    fixture.bring_up("d1");
    fixture
        .leases
        .acquire(&id("controller-b"), &id("d1"), 2_000, 1_000)
        .expect("takeover");
    fixture.stop(2_000)
}

fn stopping_a_disputed_resource(mut fixture: Fixture) -> HostingError {
    fixture.bring_up("d1");
    fixture.disputed();
    fixture.stop(4)
}

fn stopping_what_was_never_identified(mut fixture: Fixture) -> HostingError {
    fixture.fake.next_create(Dispatch::Unknown);
    fixture.bring_up("d1");
    fixture.stop(2)
}

fn retrying_a_provider_that_cannot(mut fixture: Fixture) -> HostingError {
    fixture.fake.idempotency(Idempotency::Unsupported);
    fixture.fake.next_create(Dispatch::Unknown);
    fixture.bring_up("d1");
    fixture.refuse(
        2,
        HostingCommand::Retry {
            deployment: id("d1"),
        },
    )
}

#[test]
fn every_published_refusal_is_emitted_by_some_real_sequence_of_calls() {
    for &error in HostingError::ALL {
        assert_eq!(emit(error), error, "{}", error.code());
    }
}

/// Produces each stop reason through a real sequence of public calls, exhaustively.
fn open_obligation(reason: StopReason) -> Option<StopReason> {
    let mut controller =
        Controller::new(patient_policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy");
    let mut fake = provider();
    match reason {
        StopReason::AmbiguousMutation => {
            fake.next_create(Dispatch::Unknown);
            provisioned(&mut controller, &mut fake, "d1");
        }
        StopReason::LifetimeExceeded => {
            provisioned(&mut controller, &mut fake, "d1");
            controller
                .apply(5_002, &mut fake, HostingCommand::Tick)
                .expect("tick");
        }
        StopReason::LeaseExpired => {
            let mut brief =
                Controller::new(policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy");
            provisioned(&mut brief, &mut fake, "d1");
            brief
                .apply(2_000, &mut fake, HostingCommand::Tick)
                .expect("tick");
            return state(&brief, "d1").stop_reason;
        }
        StopReason::OperatorRequest => {
            provisioned(&mut controller, &mut fake, "d1");
            controller
                .apply(
                    2,
                    &mut fake,
                    HostingCommand::RequestStop {
                        deployment: id("d1"),
                    },
                )
                .expect("request stop");
        }
        StopReason::OwnershipLost => {
            provisioned(&mut controller, &mut fake, "d1");
            controller
                .apply(2, &mut fake, HostingCommand::Observe)
                .expect("inventory");
            fake.retag(&id("pool-slot"), &id("controller-b"), 1);
            controller
                .apply(3, &mut fake, HostingCommand::Observe)
                .expect("inventory");
        }
    }
    state(&controller, "d1").stop_reason
}

#[test]
fn every_published_stop_reason_is_opened_by_some_real_sequence_of_calls() {
    for &reason in StopReason::ALL {
        assert_eq!(open_obligation(reason), Some(reason), "{}", reason.code());
    }
}

// ---------------------------------------------------------------------------------------------
// Guards that a mutation is now aimed at. Each of these deleted its own guard and survived the
// whole suite before these cases existed; `docs/verification/hosting-falsification.json` names
// the mutation and the case it kills.
// ---------------------------------------------------------------------------------------------

/// `Provision` is permitted only from `Declared`, and against a provider that does not honour a
/// repeated request id the guard is the only thing between one authorization and two bills.
///
/// The refusal is the cheap part. What this measures is the provider: no second create mutation
/// was submitted and no second resource was allocated.
#[test]
fn a_second_provision_on_one_authorization_is_refused_before_a_second_resource_is_billed() {
    let mut controller = open();
    let mut fake = provider();
    // A provider that will happily allocate again for the same request id.
    fake.idempotency(Idempotency::Unsupported);
    provisioned(&mut controller, &mut fake, "d1");
    assert_eq!(state(&controller, "d1").phase, Phase::Requested);
    assert_eq!(fake.submits(), 1);
    assert_eq!(fake.allocations(), 1);

    let again = controller.apply(
        2,
        &mut fake,
        HostingCommand::Provision {
            deployment: id("d1"),
        },
    );
    assert_eq!(
        again.err(),
        Some(HostingError::WrongPhase),
        "a create was already submitted for this authorization"
    );
    assert_eq!(
        fake.submits(),
        1,
        "a second create mutation was submitted for one authorization"
    );
    assert_eq!(
        fake.allocations(),
        1,
        "a second resource was billed against one reservation"
    );
}

/// The same guard from the other live phases: a record whose create was submitted is past the
/// point where `Provision` means anything, whatever it went on to become.
#[test]
fn provisioning_is_refused_from_every_phase_that_already_submitted_a_create() {
    for (phase, build) in [
        (Phase::Requested, 0_u8),
        (Phase::Uncertain, 1),
        (Phase::StopRequired, 2),
    ] {
        let mut controller = open();
        let mut fake = provider();
        fake.idempotency(Idempotency::Unsupported);
        if build == 1 {
            fake.next_create(Dispatch::Unknown);
        }
        provisioned(&mut controller, &mut fake, "d1");
        if build == 2 {
            controller
                .apply(
                    2,
                    &mut fake,
                    HostingCommand::RequestStop {
                        deployment: id("d1"),
                    },
                )
                .expect("request stop");
        }
        assert_eq!(state(&controller, "d1").phase, phase, "fixture phase");
        let submits = fake.submits();
        assert_eq!(
            controller
                .apply(
                    3,
                    &mut fake,
                    HostingCommand::Provision {
                        deployment: id("d1"),
                    },
                )
                .err(),
            Some(HostingError::WrongPhase),
            "provisioning from {phase:?}"
        );
        assert_eq!(
            fake.submits(),
            submits,
            "a create was submitted from {phase:?}"
        );
    }
}

/// A claim that simply ran out, with nobody taking it over.
///
/// The lease registry's epoch is untouched — no second controller ever acquired — so the stale
/// epoch comparison cannot see this. `fence`'s liveness check is the only thing refusing it, and
/// what it protects is the provider: a controller that no longer holds a claim must submit
/// nothing. The obligation the expiry opened is a separate fact and is asserted alongside, not
/// instead.
#[test]
fn a_claim_that_merely_expired_still_refuses_every_provider_mutation() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    // Granted at 1 for 1_000ms, so live at 1_001 and expired at 1_002. Nobody else acquires.
    assert!(
        controller
            .apply(
                1_001,
                &mut fake,
                HostingCommand::Stop {
                    deployment: id("d1")
                },
            )
            .is_ok(),
        "the claim is live at its expiry instant"
    );
    assert_eq!(fake.stops(), 1);

    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let expired = controller.apply(
        1_002,
        &mut fake,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert_eq!(
        expired.err(),
        Some(HostingError::LeaseLost),
        "a controller whose own claim ran out may not mutate"
    );
    assert_eq!(
        fake.stops(),
        0,
        "a stop mutation was submitted without a live claim"
    );
    let record = state(&controller, "d1");
    assert_eq!(record.phase, Phase::StopRequired);
    assert_eq!(record.stop_reason, Some(StopReason::LeaseExpired));
    assert_eq!(record.stop_evidence, None, "expiry is not evidence");
}

/// A restored record whose **requested** specification names another scope.
///
/// The observed identity is in scope here, so the `foreign-resource` check cannot see this: what
/// is out of scope is the request the record was admitted under. Both fields are checked, and
/// both are measured, because a check written as `provider != || account !=` has two halves.
#[test]
fn a_restored_record_requested_for_another_scope_is_refused() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("inventory");
    let clean = controller.snapshot();
    assert!(
        Controller::restore(policy(), Arc::clone(&registry), clean.clone(), 5).is_ok(),
        "the fixture snapshot itself must be restorable"
    );

    let mut foreign_account = clean.clone();
    foreign_account[0].requested.account = id("account-9");
    assert!(
        foreign_account[0]
            .observed
            .as_ref()
            .is_some_and(|observed| observed.key.account == id("account-1")),
        "the observed identity stays in scope, so only the requested one is out"
    );
    assert_eq!(
        Controller::restore(policy(), Arc::clone(&registry), foreign_account, 5).err(),
        Some(HostingError::InvalidSpec),
        "a record admitted for another account was reopened in this scope"
    );

    let mut foreign_provider = clean;
    foreign_provider[0].requested.provider = id("other-cloud");
    assert_eq!(
        Controller::restore(policy(), Arc::clone(&registry), foreign_provider, 5).err(),
        Some(HostingError::InvalidSpec),
        "a record admitted for another provider was reopened in this scope"
    );
}

/// First-writer-wins, through the public API, for the reasons that are *not* `ownership-lost`.
///
/// The other direction of the same rule is the adversary's
/// `an_operator_requested_stop_must_not_hide_a_later_foreign_owner_tag`. Between them the
/// precedence is asserted both ways: without this one, last-writer-wins passes; without that
/// one, a foreign owner tag is dropped and another controller's compute is destroyed.
#[test]
fn the_reason_that_opened_an_obligation_is_the_one_kept() {
    let mut controller = open();
    let mut fake = provider();
    provisioned(&mut controller, &mut fake, "d1");
    // The lease expires first and opens the obligation.
    controller
        .apply(1_002, &mut fake, HostingCommand::Tick)
        .expect("tick past the lease");
    assert_eq!(
        state(&controller, "d1").stop_reason,
        Some(StopReason::LeaseExpired)
    );
    // Then an operator asks for the stop, and the time ceiling fires as well.
    controller
        .apply(
            1_003,
            &mut fake,
            HostingCommand::RequestStop {
                deployment: id("d1"),
            },
        )
        .expect("request stop");
    assert_eq!(
        state(&controller, "d1").stop_reason,
        Some(StopReason::LeaseExpired),
        "an operator request does not rewrite why the obligation opened"
    );
    controller
        .apply(5_002, &mut fake, HostingCommand::Tick)
        .expect("tick past the deadline");
    let record = state(&controller, "d1");
    assert_eq!(
        record.stop_reason,
        Some(StopReason::LeaseExpired),
        "the time ceiling does not rewrite why the obligation opened either"
    );
    assert_eq!(record.phase, Phase::StopRequired);
}

/// `RequestStop` and `ConfirmStopped` report the phase write the table refused.
///
/// Neither command does anything but ask for one phase change, so a change the table forbids is
/// the command being refused — not the command succeeding quietly. `Declared` is the case that
/// was reported as `Ok`: nothing can be owed before a create was submitted.
#[test]
fn a_command_whose_only_phase_change_is_forbidden_is_refused_rather_than_reported_done() {
    let mut controller = open();
    let mut fake = provider();
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
    for command in [
        HostingCommand::RequestStop {
            deployment: id("d1"),
        },
        HostingCommand::ConfirmStopped {
            deployment: id("d1"),
            evidence: id("invoice-line-7"),
        },
    ] {
        assert_eq!(
            controller.apply(2, &mut fake, command.clone()).err(),
            Some(HostingError::WrongPhase),
            "{command:?}"
        );
        let record = state(&controller, "d1");
        assert_eq!(record.phase, Phase::Declared);
        assert_eq!(record.stop_reason, None);
        assert_eq!(record.stop_evidence, None);
        assert!(controller.view().totals.stop_required.is_empty());
    }
    // Cancel is the mirror: from `Declared` it is the one that is permitted.
    controller
        .apply(
            3,
            &mut fake,
            HostingCommand::Cancel {
                deployment: id("d1"),
            },
        )
        .expect("cancel");
    assert_eq!(state(&controller, "d1").phase, Phase::Cancelled);
    // And once terminal, both are refused again, by the same guard.
    for command in [
        HostingCommand::RequestStop {
            deployment: id("d1"),
        },
        HostingCommand::ConfirmStopped {
            deployment: id("d1"),
            evidence: id("invoice-line-8"),
        },
    ] {
        assert_eq!(
            controller.apply(4, &mut fake, command.clone()).err(),
            Some(HostingError::WrongPhase),
            "{command:?} on a cancelled record"
        );
    }
    assert_eq!(fake.stops(), 0);
    assert_eq!(fake.submits(), 0);
}
