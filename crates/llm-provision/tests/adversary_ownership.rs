//! Adversarial cases for the owned-resource hosting contract.
//!
//! These drive the real `Controller` through the real public `HostingProvider` seam. They differ
//! from `tests/hosting.rs` in one respect only: the provider they use reports states that
//! `FakeProvider` cannot produce but that `ObservedResource` explicitly admits — an owner label
//! without an epoch label, and a listing that does not echo the idempotency key back. Nothing
//! here opens a socket, reads a credential or allocates a cloud resource.

use std::sync::Arc;

use llm_provision::{
    Completeness, ComputeAuthorization, Controller, CreateOutcome, CreateRequest, DeploymentSpec,
    Dispatch, FakeProvider, HostingCommand, HostingError, HostingPolicy, HostingProvider,
    Identifier, Inventory, LeaseRegistry, ObservedResource, Phase, ProviderState, ResourceKey,
    StopOutcome, StopReason,
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

fn our_key() -> ResourceKey {
    ResourceKey {
        provider: id("fake"),
        account: id("account-1"),
        name: id("pool-slot"),
        incarnation: id("incarnation-1"),
    }
}

/// One listed resource, with the two ownership labels supplied independently.
///
/// `ObservedResource::owner` and `ObservedResource::epoch` are separate `Option` fields of the
/// published contract, so a provider that carries one label and not the other is a state the
/// contract admits. `FakeProvider` gates both on a single flag and cannot produce it.
fn listed(
    key: &ResourceKey,
    owner: Option<Identifier>,
    epoch: Option<u64>,
    request_id: Option<Identifier>,
) -> ObservedResource {
    ObservedResource {
        key: key.clone(),
        state: ProviderState::Running,
        ready: Some(true),
        endpoint: Some("http://scripted.invalid/one".to_owned()),
        served_model: Some(id("scripted-served-model")),
        owner,
        epoch,
        request_id,
    }
}

/// A provider whose every answer is written by the test.
#[derive(Debug, Default)]
struct Scripted {
    create_dispatch: Option<Dispatch>,
    create_answer: Option<ObservedResource>,
    listed: Vec<ObservedResource>,
    complete: bool,
    submits: u32,
    stops: u32,
    stopped_keys: Vec<ResourceKey>,
}

impl HostingProvider for Scripted {
    fn create(&mut self, _request: &CreateRequest) -> CreateOutcome {
        self.submits += 1;
        CreateOutcome {
            dispatch: self.create_dispatch.unwrap_or(Dispatch::Accepted),
            resource: self.create_answer.clone(),
        }
    }

    fn stop(&mut self, key: &ResourceKey, _epoch: u64) -> StopOutcome {
        self.stops += 1;
        self.stopped_keys.push(key.clone());
        StopOutcome {
            dispatch: Dispatch::Accepted,
            evidence: Some(id("scripted-stop-1")),
        }
    }

    fn inventory(&mut self) -> Inventory {
        Inventory {
            completeness: if self.complete {
                Completeness::Complete
            } else {
                Completeness::Partial
            },
            resources: self.listed.clone(),
        }
    }

    fn honours_idempotency_key(&self) -> bool {
        false
    }
}

fn record(controller: &Controller, name: &str) -> llm_provision::DeploymentRecord {
    let wanted = id(name);
    controller
        .view()
        .deployments
        .into_iter()
        .find(|record| record.deployment == wanted)
        .expect("declared deployment")
}

/// Declare, acquire and provision `d1` at t=1 against whatever the provider is scripted to say.
fn bring_up(controller: &mut Controller, provider: &mut dyn HostingProvider) {
    controller
        .apply(
            1,
            provider,
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
            provider,
            HostingCommand::Acquire {
                deployment: id("d1"),
            },
        )
        .expect("acquire");
    controller
        .apply(
            1,
            provider,
            HostingCommand::Provision {
                deployment: id("d1"),
            },
        )
        .expect("provision");
}

fn open() -> Controller {
    Controller::new(policy(), Arc::new(LeaseRegistry::default()), 0).expect("policy")
}

/// `controller.rs:701` says a foreign owner tag means "either way we stop mutating it", and
/// `docs/hosting.md:107` says the same. `Active -> Uncertain` is not in `TRANSITIONS`, so the
/// branch that was written for a foreign tag without a strictly newer epoch is a silent no-op.
#[test]
fn a_resource_retagged_for_another_owner_must_not_stay_active_and_unobliged() {
    let mut controller = open();
    let key = our_key();
    let mut provider = Scripted {
        complete: true,
        create_answer: Some(listed(
            &key,
            Some(id("controller-a")),
            None,
            Some(id("request-d1")),
        )),
        listed: vec![listed(
            &key,
            Some(id("controller-a")),
            None,
            Some(id("request-d1")),
        )],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);
    controller
        .apply(2, &mut provider, HostingCommand::Observe)
        .expect("first observation");
    assert_eq!(record(&controller, "d1").phase, Phase::Active);

    // The provider now reports the very same resource under somebody else's owner label, and
    // carries no epoch label at all — the two fields are independently optional.
    provider.listed = vec![listed(
        &key,
        Some(id("controller-b")),
        None,
        Some(id("request-d1")),
    )];
    controller
        .apply(3, &mut provider, HostingCommand::Observe)
        .expect("second observation");

    let seen = record(&controller, "d1");
    assert!(
        seen.phase == Phase::Disowned || seen.phase.owes_a_stop(),
        "the provider reported this resource as controller-b's; the record is still {:?} with \
         stop_reason {:?}",
        seen.phase,
        seen.stop_reason
    );
}

/// The consequence of the case above: the controller goes on to destroy the other owner's
/// compute, which is the exact failure `docs/hosting.md:16` says the contract exists to prevent.
#[test]
fn the_controller_must_not_stop_a_resource_the_provider_says_is_another_owners() {
    let mut controller = open();
    let key = our_key();
    let mut provider = Scripted {
        complete: true,
        create_answer: Some(listed(
            &key,
            Some(id("controller-a")),
            None,
            Some(id("request-d1")),
        )),
        listed: vec![listed(
            &key,
            Some(id("controller-a")),
            None,
            Some(id("request-d1")),
        )],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);
    controller
        .apply(2, &mut provider, HostingCommand::Observe)
        .expect("first observation");
    provider.listed = vec![listed(
        &key,
        Some(id("controller-b")),
        None,
        Some(id("request-d1")),
    )];
    controller
        .apply(3, &mut provider, HostingCommand::Observe)
        .expect("second observation");

    let _outcome = controller.apply(
        4,
        &mut provider,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert!(
        provider.stopped_keys.is_empty(),
        "the controller submitted {} stop mutation(s) against a resource the provider reports \
         belongs to controller-b: {:?}",
        provider.stops,
        provider.stopped_keys
    );
}

/// The same defect, reached through the fixture vocabulary the unit itself published: the
/// adapter's `Retag` step takes a free `epoch: u64`, and an epoch that is not strictly newer
/// leaves the record untouched.
#[test]
fn a_retag_at_an_epoch_that_is_not_newer_must_not_be_ignored() {
    let registry = Arc::new(LeaseRegistry::default());
    let mut controller = Controller::new(policy(), Arc::clone(&registry), 0).expect("policy");
    let mut fake = FakeProvider::new(id("fake"), id("account-1"));
    bring_up(&mut controller, &mut fake);
    controller
        .apply(2, &mut fake, HostingCommand::Observe)
        .expect("first observation");
    assert_eq!(record(&controller, "d1").phase, Phase::Active);

    fake.retag(&id("pool-slot"), &id("controller-b"), 1);
    controller
        .apply(3, &mut fake, HostingCommand::Observe)
        .expect("second observation");

    let seen = record(&controller, "d1");
    assert!(
        seen.phase == Phase::Disowned || seen.phase.owes_a_stop(),
        "retagged for controller-b at epoch 1; the record is still {:?} with stop_reason {:?}",
        seen.phase,
        seen.stop_reason
    );
}

/// `controller.rs:641` concludes absence from "no listed resource carries my request id" plus a
/// complete listing. A listing is complete when every resource in the scope is listed; it is not
/// a promise that every listed resource carries an idempotency key back, and
/// `ObservedResource::request_id` is `Option` precisely because providers may not.
#[test]
fn a_complete_listing_that_echoes_no_request_id_must_not_discharge_an_ambiguous_create() {
    let mut controller = open();
    let key = our_key();
    let mut provider = Scripted {
        complete: true,
        create_dispatch: Some(Dispatch::Unknown),
        create_answer: None,
        // The lost create did allocate. The provider lists the running resource, tagged as ours,
        // and echoes no request id.
        listed: vec![listed(&key, Some(id("controller-a")), None, None)],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);
    assert_eq!(record(&controller, "d1").phase, Phase::Uncertain);

    controller
        .apply(2, &mut provider, HostingCommand::Observe)
        .expect("observation");

    let seen = record(&controller, "d1");
    assert!(
        seen.phase.owes_a_stop(),
        "the resource is listed and running, yet the ambiguous create settled as {:?} with \
         evidence {:?}",
        seen.phase,
        seen.stop_evidence.as_ref().map(Identifier::as_str)
    );
}

/// `HostingError::ForeignResource` is published in the generated `ALL` inventory and documented
/// as "the observed resource is not the one owned here". Nothing constructs it: a create answer
/// is recorded as this record's own observation without any check that it names this scope.
#[test]
fn a_create_answer_naming_another_account_must_not_be_adopted_as_ours() {
    let mut controller = open();
    let foreign = ResourceKey {
        provider: id("fake"),
        account: id("account-9"),
        name: id("someone-elses-slot"),
        incarnation: id("incarnation-9"),
    };
    let mut provider = Scripted {
        complete: true,
        create_answer: Some(listed(
            &foreign,
            Some(id("controller-z")),
            Some(7),
            Some(id("request-d1")),
        )),
        listed: vec![listed(
            &foreign,
            Some(id("controller-z")),
            Some(7),
            Some(id("request-d1")),
        )],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);

    // Rewritten by the coordinator. As written this case required the foreign answer to be
    // recorded as this record's observation AND required that recorded account to be ours. The
    // two are satisfiable only by storing the answer with its account rewritten — inventing a
    // fact the provider contradicted, which is the defect this repository exists to prevent.
    // The intent is kept and is what the implementation now does: the answer is refused, nothing
    // is adopted, and the obligation stays open under `ownership-lost`.
    //
    // The rewrite also carried an assertion that every stopped key named `account-1`. This case
    // never applies `Stop`, so that assertion ranged over an empty list and decided nothing; it
    // is removed rather than dressed up. What it was reaching for is decided where a stop is
    // actually submitted — `a_stop_must_never_be_addressed_to_another_accounts_resource` below,
    // and `hosting.rs`'s
    // `a_create_answer_outside_this_scope_is_not_adopted_and_nothing_is_addressed_to_it`.
    let seen = record(&controller, "d1");
    assert!(
        seen.observed.is_none(),
        "a create answer naming another account was adopted: {:?}",
        seen.observed.clone().map(|observed| observed.key)
    );
    assert_eq!(
        seen.stop_reason,
        Some(StopReason::OwnershipLost),
        "an answer we cannot account for leaves the obligation open, and says why"
    );
    assert!(seen.phase.owes_a_stop(), "phase was {:?}", seen.phase);
    assert_eq!(provider.submits, 1, "exactly one create was submitted");
}

/// And then addresses a stop mutation to it.
#[test]
fn a_stop_must_never_be_addressed_to_another_accounts_resource() {
    let mut controller = open();
    let foreign = ResourceKey {
        provider: id("fake"),
        account: id("account-9"),
        name: id("someone-elses-slot"),
        incarnation: id("incarnation-9"),
    };
    let mut provider = Scripted {
        complete: true,
        create_answer: Some(listed(
            &foreign,
            Some(id("controller-z")),
            Some(7),
            Some(id("request-d1")),
        )),
        listed: vec![listed(
            &foreign,
            Some(id("controller-z")),
            Some(7),
            Some(id("request-d1")),
        )],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);
    controller
        .apply(
            2,
            &mut provider,
            HostingCommand::RequestStop {
                deployment: id("d1"),
            },
        )
        .expect("request stop");
    let outcome = controller.apply(
        3,
        &mut provider,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );

    // `all` over `stopped_keys` was the assertion here too, and `stopped_keys` is empty exactly
    // when the contract holds — so it passed whether or not the refusal existed. The refusal is
    // named instead. `foreign-resource` and not `ambiguous-mutation`: the record has no observed
    // identity either, and answering with the weaker of the two would hide the stronger fact.
    assert_eq!(
        outcome,
        Err(HostingError::ForeignResource),
        "a stop was not refused as addressed to a resource outside this scope"
    );
    assert!(
        provider.stopped_keys.is_empty(),
        "a stop mutation was addressed to {:?}",
        provider
            .stopped_keys
            .iter()
            .map(|key| key.account.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(provider.stops, 0);
}

// ---------------------------------------------------------------------------------------------
// The two scope guards `Stop` rests on. `controller.rs` deliberately carries no scope check of
// its own, on the argument that an out-of-scope identity cannot reach a record at all — three
// legs hold that argument up, and two of them were measured by nothing. These are those two;
// the third is `restore`, in `hosting.rs`.
// ---------------------------------------------------------------------------------------------

/// `adopt`'s scope guard, reached the only way it can be: through the request-id route.
///
/// The exact-key route cannot get here, because a record's key is already in scope. The
/// request-id route searches the **whole** listing for anything carrying our idempotency key, so
/// a provider that answers with a resource in another account hands it straight to `adopt`. That
/// resource is not ours whatever key it carries: it is never recorded as this record's
/// observation, and the obligation stays open rather than being resolved by somebody else's
/// compute.
#[test]
fn a_resource_in_another_scope_carrying_our_request_id_is_never_adopted() {
    let mut controller = open();
    let foreign = ResourceKey {
        provider: id("fake"),
        account: id("account-9"),
        name: id("pool-slot"),
        incarnation: id("incarnation-9"),
    };
    let mut provider = Scripted {
        complete: true,
        // The create answer was lost, so the record carries no key of its own.
        create_dispatch: Some(Dispatch::Unknown),
        create_answer: None,
        // And the listing echoes our request id from a resource in another account.
        listed: vec![listed(
            &foreign,
            Some(id("controller-a")),
            Some(1),
            Some(id("request-d1")),
        )],
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);
    let unresolved = record(&controller, "d1");
    assert_eq!(unresolved.phase, Phase::Uncertain, "fixture precondition");
    assert!(unresolved.observed.is_none(), "fixture precondition");

    controller
        .apply(2, &mut provider, HostingCommand::Observe)
        .expect("observation");

    let seen = record(&controller, "d1");
    assert!(
        seen.observed.is_none(),
        "a resource in account-9 was adopted as ours: {:?}",
        seen.observed.clone().map(|observed| observed.key)
    );
    assert_eq!(
        seen.stop_reason,
        Some(StopReason::OwnershipLost),
        "a listing we cannot account for leaves the obligation open, and says why"
    );
    assert!(seen.phase.owes_a_stop(), "phase was {:?}", seen.phase);

    // And the consequence the guard exists for: the stop is never addressed to account-9.
    let outcome = controller.apply(
        3,
        &mut provider,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert_eq!(outcome, Err(HostingError::ForeignResource));
    assert!(
        provider.stopped_keys.is_empty(),
        "a stop was addressed to {:?}",
        provider
            .stopped_keys
            .iter()
            .map(|key| key.account.as_str())
            .collect::<Vec<_>>()
    );
}

/// `settle_create`'s owner guard, on an answer whose **key** is perfectly in scope.
///
/// The scope check cannot see this one: the provider answered about a resource at our provider,
/// our account and the name we asked for, and labelled it for `controller-z`. `FakeProvider`
/// cannot produce it, because it tags every resource it creates with the requesting owner — so
/// no fixture reached this branch and neutralising it changed nothing. An answer labelled for
/// somebody else is still an answer we cannot account for: nothing is adopted, and the create we
/// submitted may still have allocated, so the obligation stays open.
#[test]
fn a_create_answer_labelled_for_another_controller_is_not_adopted() {
    let mut controller = open();
    let key = our_key();
    let mut provider = Scripted {
        complete: true,
        create_answer: Some(listed(
            &key,
            Some(id("controller-z")),
            Some(7),
            Some(id("request-d1")),
        )),
        ..Scripted::default()
    };
    bring_up(&mut controller, &mut provider);

    let seen = record(&controller, "d1");
    assert_eq!(provider.submits, 1, "the create really was submitted");
    assert!(
        seen.observed.is_none(),
        "an answer labelled for controller-z was adopted as ours: {:?}",
        seen.observed.clone().map(|observed| observed.key)
    );
    assert_eq!(seen.phase, Phase::Uncertain);
    assert_eq!(
        seen.stop_reason,
        Some(StopReason::OwnershipLost),
        "the reason must name the ownership, not the ambiguity"
    );

    let outcome = controller.apply(
        2,
        &mut provider,
        HostingCommand::Stop {
            deployment: id("d1"),
        },
    );
    assert_eq!(outcome, Err(HostingError::ForeignResource));
    assert_eq!(provider.stopped_keys, [] as [llm_provision::ResourceKey; 0]);
}
