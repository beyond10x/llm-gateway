//! Owned-resource hosting conformance observations.
//!
//! This module translates an authored fixture program, and each `OwnedResource` lifecycle
//! command, into calls on the real `b10x-llm-provision` public API and reports what those calls
//! returned. It reimplements no lifecycle rule, reads no suite, and branches on no scenario
//! name: every phase, refusal, counter and obligation below comes back out of the library.

use std::{collections::BTreeMap, sync::Arc};

use ess_conformance::target::TargetError;
use llm_provision::{
    AmbiguousCreate, Completeness, ComputeAuthorization, Controller, DeploymentRecord,
    DeploymentSpec, Dispatch, FakeProvider, HostingCommand, HostingError, HostingPolicy,
    Idempotency, Identifier, LeaseRegistry, Phase, StopReason, TRANSITIONS,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::target::Observed;

/// Every view name this domain answers through `query_view`.
pub const VIEWS: &[&str] = &["llm-gateway.hosting.LastExecution"];

/// The largest fixture program this adapter will run.
const MAX_STEPS: usize = 128;
/// The largest number of simultaneous acquirers one concurrency step may spawn.
const MAX_CONCURRENT: usize = 32;
/// The largest fixture document this adapter will parse.
const MAX_PROGRAM_BYTES: usize = 64 * 1024;

/// Whether a fixture document is inside the size this adapter will parse.
fn within_size(program_json: &str) -> bool {
    program_json.len() <= MAX_PROGRAM_BYTES
}

/// Observe one command of this domain, or `None` when the command belongs to another.
pub fn observe(command: &str, input: &Value) -> Option<Result<Observed, TargetError>> {
    if command != "llm-gateway.hosting.Exercise" {
        return None;
    }
    Some(exercise(input))
}

fn unavailable(error: impl std::fmt::Display) -> TargetError {
    TargetError::unavailable("hosting observation", error.to_string())
}

fn exercise(input: &Value) -> Result<Observed, TargetError> {
    let request: Exercise = serde_json::from_value(input.clone()).map_err(unavailable)?;
    Ok(Observed {
        facts: run(&request.program_json),
        view: "llm-gateway.hosting.LastExecution",
        event: "llm-gateway.hosting.Exercised",
        field: "valid_program",
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exercise {
    program_json: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Program {
    policy: PolicyInput,
    #[serde(default)]
    provider: ProviderInput,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyInput {
    controller: String,
    provider: String,
    account: String,
    ledger: String,
    max_active: u32,
    max_lifetime_ms: u64,
    lease_ms: u64,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct ProviderInput {
    idempotency: Option<String>,
    completeness: Option<String>,
    ambiguous: Option<String>,
    /// `ready`, `not-ready` or `unreported`. Absent leaves the current setting alone;
    /// `unreported` is the provider that answers nothing about readiness at all, which is
    /// a different fact from a provider that answers `not-ready`.
    readiness: Option<String>,
    endpoint: Option<bool>,
    served_model: Option<bool>,
    owner_tag: Option<bool>,
    /// Whether a listed resource echoes the idempotency key it was created with.
    request_id: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecInput {
    id: String,
    provider: String,
    account: String,
    model: String,
    image: String,
    resource_name: String,
    requested_lifetime_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationInput {
    ledger: String,
    reservation: String,
}

#[derive(Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case", deny_unknown_fields)]
enum Step {
    Tick {
        at_ms: u64,
    },
    Validate {
        at_ms: u64,
        spec: SpecInput,
    },
    Declare {
        at_ms: u64,
        spec: SpecInput,
        request_id: String,
        authorization: AuthorizationInput,
    },
    Acquire {
        at_ms: u64,
        deployment: String,
    },
    Renew {
        at_ms: u64,
        deployment: String,
    },
    Release {
        at_ms: u64,
        deployment: String,
    },
    Provision {
        at_ms: u64,
        deployment: String,
    },
    Retry {
        at_ms: u64,
        deployment: String,
    },
    Observe {
        at_ms: u64,
    },
    Connect {
        at_ms: u64,
        deployment: String,
    },
    Disconnect {
        at_ms: u64,
        deployment: String,
    },
    RequestStop {
        at_ms: u64,
        deployment: String,
    },
    Stop {
        at_ms: u64,
        deployment: String,
    },
    ConfirmStopped {
        at_ms: u64,
        deployment: String,
        evidence: String,
    },
    Cancel {
        at_ms: u64,
        deployment: String,
    },
    /// Drop the controller and rebuild it from its own durable records, optionally under a
    /// different policy — an operator may tighten a ceiling between restarts.
    Restart {
        at_ms: u64,
        #[serde(default)]
        policy: Option<PolicyInput>,
    },
    /// Race several owners for one deployment's lease, on real threads.
    ConcurrentAcquire {
        at_ms: u64,
        deployment: String,
        owners: Vec<String>,
        lease_ms: u64,
    },
    /// Let a different controller take the lease, as a takeover would.
    ForeignAcquire {
        at_ms: u64,
        deployment: String,
        owner: String,
        lease_ms: u64,
    },
    /// Reconfigure what the fake provider reports from here on.
    Provider {
        settings: ProviderInput,
    },
    NextCreate {
        dispatch: String,
    },
    NextStop {
        dispatch: String,
    },
    Vanish {
        name: String,
    },
    Terminate {
        name: String,
    },
    ReuseName {
        name: String,
        owner: String,
    },
    Retag {
        name: String,
        owner: String,
        epoch: u64,
    },
}

fn ident(value: &str) -> Result<Identifier, String> {
    Identifier::new(value).map_err(|error| error.to_string())
}

fn dispatch(value: &str) -> Result<Dispatch, String> {
    Dispatch::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.code() == value)
        .ok_or_else(|| format!("unknown dispatch {value}"))
}

fn from_code<T: Copy + 'static>(all: &[T], code: fn(T) -> &'static str, value: &str) -> Option<T> {
    all.iter()
        .copied()
        .find(|candidate| code(*candidate) == value)
}

impl PolicyInput {
    fn build(&self) -> Result<HostingPolicy, String> {
        Ok(HostingPolicy {
            controller: ident(&self.controller)?,
            provider: ident(&self.provider)?,
            account: ident(&self.account)?,
            ledger: ident(&self.ledger)?,
            max_active: self.max_active,
            max_lifetime_ms: self.max_lifetime_ms,
            lease_ms: self.lease_ms,
        })
    }
}

impl SpecInput {
    fn build(&self) -> Result<DeploymentSpec, String> {
        Ok(DeploymentSpec {
            id: ident(&self.id)?,
            provider: ident(&self.provider)?,
            account: ident(&self.account)?,
            model: ident(&self.model)?,
            image: ident(&self.image)?,
            resource_name: ident(&self.resource_name)?,
            requested_lifetime_ms: self.requested_lifetime_ms,
        })
    }
}

impl AuthorizationInput {
    fn build(&self) -> Result<ComputeAuthorization, String> {
        Ok(ComputeAuthorization {
            ledger: ident(&self.ledger)?,
            reservation: ident(&self.reservation)?,
        })
    }
}

impl ProviderInput {
    fn apply(&self, fake: &mut FakeProvider) -> Result<(), String> {
        if let Some(value) = &self.idempotency {
            fake.idempotency(
                from_code(Idempotency::ALL, Idempotency::code, value)
                    .ok_or_else(|| format!("unknown idempotency {value}"))?,
            );
        }
        if let Some(value) = &self.completeness {
            fake.inventory_completeness(
                from_code(Completeness::ALL, Completeness::code, value)
                    .ok_or_else(|| format!("unknown completeness {value}"))?,
            );
        }
        if let Some(value) = &self.ambiguous {
            fake.ambiguous_create(
                from_code(AmbiguousCreate::ALL, AmbiguousCreate::code, value)
                    .ok_or_else(|| format!("unknown ambiguous effect {value}"))?,
            );
        }
        if let Some(value) = &self.readiness {
            fake.report_readiness(match value.as_str() {
                "ready" => Some(true),
                "not-ready" => Some(false),
                "unreported" => None,
                other => return Err(format!("unknown readiness {other}")),
            });
        }
        if let Some(report) = self.endpoint {
            fake.report_endpoint(report);
        }
        if let Some(report) = self.served_model {
            fake.report_served_model(report);
        }
        if let Some(tag) = self.owner_tag {
            fake.tag_owner(tag);
        }
        if let Some(echo) = self.request_id {
            fake.echo_request_id(echo);
        }
        Ok(())
    }
}

/// The facts of a program that never ran. Every count is absent, because no provider was
/// ever built — an unreported count is not a count of zero.
fn refused_program() -> Value {
    json!({
        "valid_program": false, "error_code": null, "results": [],
        "allocations": null, "submits": null, "stops": null, "lists": null,
        "concurrent_granted": [], "concurrent_errors": [],
        "controller": null, "ledger": null, "max_active": null, "now_ms": null,
        "totals": null, "deployments": [], "transitions": transitions(),
    })
}

/// The transition table the library actually enforces, read from its published constant.
fn transitions() -> Value {
    json!(
        TRANSITIONS
            .iter()
            .map(|(from, to)| json!({"from": from.code(), "to": to.code()}))
            .collect::<Vec<_>>()
    )
}

fn run(program_json: &str) -> Value {
    if !within_size(program_json) {
        return refused_program();
    }
    let Ok(program) = serde_json::from_str::<Program>(program_json) else {
        return refused_program();
    };
    if program.steps.len() > MAX_STEPS
        || program.steps.iter().any(
            |step| matches!(step, Step::ConcurrentAcquire { owners, .. } if owners.len() > MAX_CONCURRENT),
        )
    {
        return refused_program();
    }
    let Ok(policy) = program.policy.build() else {
        return refused_program();
    };
    let mut facts = refused_program();
    facts["valid_program"] = json!(true);

    let leases = Arc::new(LeaseRegistry::default());
    let mut fake = FakeProvider::new(policy.provider.clone(), policy.account.clone());
    if program.provider.apply(&mut fake).is_err() {
        return refused_program();
    }
    let controller = match Controller::new(policy.clone(), Arc::clone(&leases), 0) {
        Ok(controller) => controller,
        Err(error) => {
            facts["error_code"] = json!(error.code());
            // The provider exists and was never called. Nought calls is a count, not an
            // absence, and reporting it as absent would be as wrong as the other way round.
            counters(&fake, &mut facts);
            return facts;
        }
    };
    let mut run = Run {
        controller,
        fake,
        policy,
        leases,
        results: Vec::new(),
        granted: Vec::new(),
        errors: Vec::new(),
    };
    for step in program.steps {
        // A step this adapter cannot build is a malformed fixture, not an observation. It
        // reports no partial run at all rather than a half-run that reads like a result.
        if run.step(step).is_err() {
            return refused_program();
        }
    }
    run.report(&mut facts);
    facts
}

/// The provider-side call counts, which exist exactly when a provider does.
fn counters(fake: &FakeProvider, facts: &mut Value) {
    facts["allocations"] = json!(fake.allocations());
    facts["submits"] = json!(fake.submits());
    facts["stops"] = json!(fake.stops());
    facts["lists"] = json!(fake.lists());
}

struct Run {
    controller: Controller,
    fake: FakeProvider,
    policy: HostingPolicy,
    leases: Arc<LeaseRegistry>,
    results: Vec<String>,
    granted: Vec<usize>,
    errors: Vec<String>,
}

impl Run {
    fn apply(&mut self, at_ms: u64, command: HostingCommand) {
        match self.controller.apply(at_ms, &mut self.fake, command) {
            Ok(_) => self.results.push("ok".into()),
            Err(error) => self.results.push(format!("err:{}", error.code())),
        }
    }

    fn step(&mut self, step: Step) -> Result<(), String> {
        match step {
            Step::Tick { at_ms } => self.apply(at_ms, HostingCommand::Tick),
            Step::Validate { at_ms, spec } => {
                let spec = spec.build()?;
                self.apply(at_ms, HostingCommand::Validate { spec });
            }
            Step::Declare {
                at_ms,
                spec,
                request_id,
                authorization,
            } => {
                let command = HostingCommand::Declare {
                    spec: spec.build()?,
                    request_id: ident(&request_id)?,
                    authorization: authorization.build()?,
                };
                self.apply(at_ms, command);
            }
            Step::Acquire { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Acquire { deployment });
            }
            Step::Renew { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Renew { deployment });
            }
            Step::Release { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Release { deployment });
            }
            Step::Provision { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Provision { deployment });
            }
            Step::Retry { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Retry { deployment });
            }
            Step::Observe { at_ms } => self.apply(at_ms, HostingCommand::Observe),
            Step::Connect { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Connect { deployment });
            }
            Step::Disconnect { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Disconnect { deployment });
            }
            Step::RequestStop { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::RequestStop { deployment });
            }
            Step::Stop { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Stop { deployment });
            }
            Step::ConfirmStopped {
                at_ms,
                deployment,
                evidence,
            } => {
                let command = HostingCommand::ConfirmStopped {
                    deployment: ident(&deployment)?,
                    evidence: ident(&evidence)?,
                };
                self.apply(at_ms, command);
            }
            Step::Cancel { at_ms, deployment } => {
                let deployment = ident(&deployment)?;
                self.apply(at_ms, HostingCommand::Cancel { deployment });
            }
            Step::Restart { at_ms, policy } => {
                let policy = match policy {
                    Some(policy) => Some(policy.build()?),
                    None => None,
                };
                self.restart(at_ms, policy);
            }
            Step::ConcurrentAcquire {
                at_ms,
                deployment,
                owners,
                lease_ms,
            } => self.concurrent(at_ms, &ident(&deployment)?, &owners, lease_ms)?,
            Step::ForeignAcquire {
                at_ms,
                deployment,
                owner,
                lease_ms,
            } => {
                let outcome =
                    self.leases
                        .acquire(&ident(&owner)?, &ident(&deployment)?, at_ms, lease_ms);
                self.record(outcome.map(|_| ()));
            }
            other => return self.fixture(other),
        }
        Ok(())
    }

    /// Steps that reconfigure the fake provider rather than drive the controller.
    fn fixture(&mut self, step: Step) -> Result<(), String> {
        match step {
            Step::Provider { settings } => settings.apply(&mut self.fake)?,
            Step::NextCreate { dispatch: value } => self.fake.next_create(dispatch(&value)?),
            Step::NextStop { dispatch: value } => self.fake.next_stop(dispatch(&value)?),
            Step::Vanish { name } => self.fake.vanish(&ident(&name)?),
            Step::Terminate { name } => self.fake.terminate(&ident(&name)?),
            Step::ReuseName { name, owner } => {
                self.fake.reuse_name(&ident(&name)?, &ident(&owner)?);
            }
            Step::Retag { name, owner, epoch } => {
                self.fake.retag(&ident(&name)?, &ident(&owner)?, epoch);
            }
            _ => return Err("step is not a fixture step".to_owned()),
        }
        self.results.push("fixture-ready".into());
        Ok(())
    }

    fn record(&mut self, outcome: Result<(), HostingError>) {
        match outcome {
            Ok(()) => self.results.push("ok".into()),
            Err(error) => self.results.push(format!("err:{}", error.code())),
        }
    }

    /// A restart is the library's own snapshot/restore pair: no storage, no I/O.
    fn restart(&mut self, at_ms: u64, policy: Option<HostingPolicy>) {
        let policy = policy.unwrap_or_else(|| self.policy.clone());
        let snapshot = self.controller.snapshot();
        match Controller::restore(policy.clone(), Arc::clone(&self.leases), snapshot, at_ms) {
            Ok(controller) => {
                self.controller = controller;
                self.policy = policy;
                self.results.push("ok".into());
            }
            // A refused restore leaves the controller that was already open untouched.
            Err(error) => self.results.push(format!("err:{}", error.code())),
        }
    }

    /// Race real threads for one lease and report how many were granted.
    fn concurrent(
        &mut self,
        at_ms: u64,
        deployment: &Identifier,
        owners: &[String],
        lease_ms: u64,
    ) -> Result<(), String> {
        let owners: Vec<Identifier> = owners
            .iter()
            .map(|owner| ident(owner))
            .collect::<Result<_, _>>()?;
        let barrier = Arc::new(std::sync::Barrier::new(owners.len().max(1)));
        let outcomes = std::thread::scope(|scope| {
            let threads: Vec<_> = owners
                .into_iter()
                .map(|owner| {
                    let barrier = Arc::clone(&barrier);
                    let leases = Arc::clone(&self.leases);
                    scope.spawn(move || {
                        barrier.wait();
                        leases.acquire(&owner, deployment, at_ms, lease_ms)
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().map_err(|_| "concurrent fixture panicked"))
                .collect::<Result<Vec<_>, _>>()
        })?;
        let mut granted = 0;
        let mut errors = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok(_) => granted += 1,
                Err(error) => errors.push(format!("err:{}", error.code())),
            }
        }
        errors.sort();
        self.errors.extend(errors);
        self.granted.push(granted);
        self.results.push("concurrent-observed".into());
        Ok(())
    }

    fn report(&self, facts: &mut Value) {
        let view = self.controller.view();
        facts["results"] = json!(self.results);
        counters(&self.fake, facts);
        facts["concurrent_granted"] = json!(self.granted);
        facts["concurrent_errors"] = json!(self.errors);
        facts["controller"] = json!(view.policy.controller.as_str());
        facts["ledger"] = json!(view.policy.ledger.as_str());
        facts["max_active"] = json!(view.policy.max_active);
        facts["now_ms"] = json!(view.now_ms.to_string());
        facts["totals"] = json!({
            "active_count": view.totals.active_count,
            "unknown_count": view.totals.unknown_count,
            "stop_required": view
                .totals
                .stop_required
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>(),
            "transferred": view
                .totals
                .transferred
                .iter()
                .map(Identifier::as_str)
                .collect::<Vec<_>>(),
            "at_capacity": view.totals.at_capacity,
        });
        facts["deployments"] = json!(
            view.deployments
                .iter()
                .map(deployment_fact)
                .collect::<Vec<_>>()
        );
    }
}

fn deployment_fact(record: &DeploymentRecord) -> Value {
    let observed = record.observed.as_ref();
    let lease = record.lease.as_ref();
    json!({
        "id": record.deployment.as_str(),
        "phase": phase(record.phase),
        "stop_reason": record.stop_reason.map(llm_provision::StopReason::code),
        "stop_evidence": record.stop_evidence.as_ref().map(Identifier::as_str),
        "connected": record.connected,
        "requested_model": record.requested.model.as_str(),
        "requested_image": record.requested.image.as_str(),
        "requested_name": record.requested.resource_name.as_str(),
        "requested_lifetime_ms": record.requested.requested_lifetime_ms.to_string(),
        "request_id": record.request_id.as_str(),
        "ledger": record.authorization.ledger.as_str(),
        "reservation": record.authorization.reservation.as_str(),
        "observed_name": observed.map(|it| it.key.name.as_str()),
        "observed_incarnation": observed.map(|it| it.key.incarnation.as_str()),
        "observed_state": observed.and_then(|it| it.state).map(llm_provision::ProviderState::code),
        "observed_ready": observed.and_then(|it| it.ready),
        "observed_endpoint": observed.and_then(|it| it.endpoint.as_deref()),
        "observed_model": observed.and_then(|it| it.served_model.as_ref()).map(Identifier::as_str),
        "lease_owner": lease.map(|it| it.owner.as_str()),
        "lease_epoch": lease.map(|it| it.epoch),
        "lease_expires_at_ms": lease.map(|it| it.expires_at_ms.to_string()),
    })
}

fn phase(phase: Phase) -> &'static str {
    phase.code()
}

// --- the OwnedResource lifecycle --------------------------------------------------------------
//
// The commands `Declare`, `Provision`, `Retry`, `ObserveListed`, `ObserveTakenOver`,
// `ObserveUnlisted`, `RequestStop`, `ConfirmStopped` and `Cancel` drive one record each through
// `Controller::apply`; the three `Observe*` are `HostingCommand::Observe` against three
// listings. Which declared outcome a command
// took is read off what the controller answered and the phase it left the record in; the
// specification's lifecycle is never consulted here, so a declared transition the controller
// does not make fails its scenario instead of being reported as made.

/// The view the lifecycle commands answer through.
pub const LIFECYCLE_VIEW: &str = "llm-gateway.hosting.OwnedResources";

const RESOURCE_DECLARED: &str = "llm-gateway.hosting.ResourceDeclared";
const PHASE_MOVED: &str = "llm-gateway.hosting.PhaseMoved";
const LIFECYCLE_CONTROLLER: &str = "lifecycle-controller";
/// The controller an `ObserveTakenOver` listing names as the resource's new owner.
const LIFECYCLE_SUCCESSOR: &str = "lifecycle-successor";
const LIFECYCLE_PROVIDER: &str = "fake";
const LIFECYCLE_ACCOUNT: &str = "account-1";
/// The account an `AcceptedOutsideScope` create answer names: same provider, not this scope.
const FOREIGN_ACCOUNT: &str = "account-2";
const LIFECYCLE_LEDGER: &str = "scope";
/// The scenario clock moves one millisecond per command, so within an hour of it no lease
/// expires and no lifetime ceiling fires: the phase alone decides every command.
const LIFECYCLE_HOUR_MS: u64 = 3_600_000;

/// One lifecycle command's answer, in the specification's words.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    /// The declared outcome the controller's answer is, or `None` when it is none of them.
    pub outcome: Option<&'static str>,
    /// The declared error a refusal carries.
    pub error: Option<&'static str>,
    /// The notification published by an outcome that created or moved a record.
    pub event: Option<&'static str>,
    /// The record the command named.
    pub id: String,
}

/// The records the lifecycle commands declared in one scenario, each with its own controller,
/// fake provider and lease registry, so a command on one record cannot move another.
pub struct Lifecycle {
    records: BTreeMap<String, Hosted>,
    now_ms: u64,
}

struct Hosted {
    controller: Controller,
    provider: FakeProvider,
    leases: Arc<LeaseRegistry>,
}

fn lifecycle_policy() -> Result<HostingPolicy, String> {
    Ok(HostingPolicy {
        controller: ident(LIFECYCLE_CONTROLLER)?,
        provider: ident(LIFECYCLE_PROVIDER)?,
        account: ident(LIFECYCLE_ACCOUNT)?,
        ledger: ident(LIFECYCLE_LEDGER)?,
        max_active: 1,
        max_lifetime_ms: LIFECYCLE_HOUR_MS,
        lease_ms: LIFECYCLE_HOUR_MS,
    })
}

impl Hosted {
    /// A controller holding no record. A command naming an identity no record carries is
    /// applied to one, so its `missing` is the controller's own answer.
    fn empty() -> Result<Self, String> {
        let policy = lifecycle_policy()?;
        let leases = Arc::new(LeaseRegistry::default());
        let mut provider = FakeProvider::new(policy.provider.clone(), policy.account.clone());
        // A create whose answer is lost did allocate, so an uncertain record has a resource
        // behind it for `Retry` to find under its request id and for `Observe` to list.
        provider.ambiguous_create(AmbiguousCreate::Allocates);
        let controller =
            Controller::new(policy, Arc::clone(&leases), 0).map_err(|error| error.to_string())?;
        Ok(Self {
            controller,
            provider,
            leases,
        })
    }

    fn record(&self, deployment: &Identifier) -> Option<DeploymentRecord> {
        self.controller
            .snapshot()
            .into_iter()
            .find(|record| &record.deployment == deployment)
    }

    fn apply(&mut self, now_ms: u64, command: HostingCommand) -> Result<(), HostingError> {
        self.controller
            .apply(now_ms, &mut self.provider, command)
            .map(|_| ())
    }
}

/// Answer one lifecycle command, or `None` when the command is not one.
pub fn lifecycle(
    state: &mut Lifecycle,
    command: &str,
    input: &Value,
) -> Option<Result<Answer, TargetError>> {
    const COMMANDS: &[&str] = &[
        "Declare",
        "Provision",
        "Retry",
        "ObserveListed",
        "ObserveTakenOver",
        "ObserveUnlisted",
        "RequestStop",
        "ConfirmStopped",
        "Cancel",
    ];
    let command = command
        .strip_prefix("llm-gateway.hosting.")
        .filter(|name| COMMANDS.contains(name))?;
    Some(state.answer(command, input).map_err(unavailable))
}

fn text<'a>(input: &'a Value, field: &str) -> Result<&'a str, String> {
    input[field]
        .as_str()
        .ok_or_else(|| format!("input field {field} is not a string"))
}

/// The provider's answer to the next create, by the variant names `CreateAnswer` and
/// `RetryAnswer` share with `Dispatch`.
fn dispatch_named(name: &str) -> Result<Dispatch, String> {
    match name {
        "Accepted" => Ok(Dispatch::Accepted),
        "Unknown" => Ok(Dispatch::Unknown),
        "Rejected" => Ok(Dispatch::Rejected),
        "NotSent" => Ok(Dispatch::NotSent),
        other => Err(format!("unknown create answer {other}")),
    }
}

/// A refusal of the controller, as the declared outcome and error that name it.
fn refused(id: &str, error: HostingError) -> Answer {
    let (outcome, error) = match error {
        HostingError::WrongPhase => ("wrong-phase", "llm-gateway.hosting.WrongPhase"),
        HostingError::Missing => ("missing", "llm-gateway.hosting.Missing"),
        HostingError::Duplicate => ("duplicate", "llm-gateway.hosting.Duplicate"),
        // A refusal the lifecycle commands declare no outcome for.
        _ => {
            return Answer {
                outcome: None,
                error: None,
                event: None,
                id: id.to_owned(),
            };
        }
    };
    Answer {
        outcome: Some(outcome),
        error: Some(error),
        event: None,
        id: id.to_owned(),
    }
}

/// Which declared outcome an accepted command took, from the record before and after it.
fn classify(
    command: &str,
    before: Option<&DeploymentRecord>,
    after: Option<&DeploymentRecord>,
    now_ms: u64,
) -> Option<&'static str> {
    use Phase::{
        Active, Cancelled, Declared, Disowned, Requested, StopRequired, Stopped, Uncertain,
    };
    let from = before.map(|record| record.phase);
    let to = after.map(|record| record.phase);
    let lost_ownership =
        after.is_some_and(|record| record.stop_reason == Some(StopReason::OwnershipLost));
    let seen_now = after.is_some_and(|record| {
        record
            .observed
            .as_ref()
            .is_some_and(|observed| observed.observed_at_ms == now_ms)
    });
    let outcome = match (command, from, to) {
        ("Provision", Some(Declared), Some(Requested))
        | ("Retry", Some(Uncertain), Some(Requested)) => "accepted",
        ("Provision", Some(Declared), Some(Uncertain)) if lost_ownership => "misanswered",
        ("Provision", Some(Declared), Some(Uncertain))
        | ("Retry", Some(Uncertain), Some(Uncertain)) => "lost",
        ("Provision", Some(Declared), Some(Cancelled)) => "refused",
        ("ObserveListed", Some(_), Some(Active)) if seen_now => "adopted",
        ("ObserveTakenOver", Some(from), Some(Disowned)) if from != Disowned => "taken-over",
        ("ObserveUnlisted", Some(Requested), Some(Requested)) => "unconfirmed",
        ("ObserveListed" | "ObserveTakenOver" | "ObserveUnlisted", from, to) if from == to => {
            "unchanged"
        }
        ("RequestStop", Some(_), Some(StopRequired)) => "obliged",
        ("ConfirmStopped", Some(_), Some(Stopped)) => "confirmed",
        ("Cancel", Some(Declared), Some(Cancelled)) => "cancelled",
        _ => return None,
    };
    Some(outcome)
}

impl Lifecycle {
    /// No record yet: the state at the start of every scenario.
    pub const fn new() -> Self {
        Self {
            records: BTreeMap::new(),
            now_ms: 0,
        }
    }

    fn answer(&mut self, command: &str, input: &Value) -> Result<Answer, String> {
        let id = text(input, "id")?;
        let deployment = ident(id)?;
        self.now_ms = self.now_ms.saturating_add(1);
        let now_ms = self.now_ms;
        if command == "Declare" {
            return self.declare(id, deployment, input, now_ms);
        }
        let mut spare = None;
        let hosted = match self.records.get_mut(id) {
            Some(hosted) => hosted,
            None => spare.insert(Hosted::empty()?),
        };
        let before = hosted.record(&deployment);
        let applied = match command {
            "Provision" => {
                let command = HostingCommand::Provision {
                    deployment: deployment.clone(),
                };
                match text(input, "answer")? {
                    "AcceptedOutsideScope" => {
                        // The provider accepts and answers about a resource in another account.
                        let mut elsewhere =
                            FakeProvider::new(ident(LIFECYCLE_PROVIDER)?, ident(FOREIGN_ACCOUNT)?);
                        hosted
                            .controller
                            .apply(now_ms, &mut elsewhere, command)
                            .map(|_| ())
                    }
                    answer => {
                        hosted.provider.next_create(dispatch_named(answer)?);
                        hosted.apply(now_ms, command)
                    }
                }
            }
            "Retry" => {
                hosted
                    .provider
                    .next_create(dispatch_named(text(input, "answer")?)?);
                hosted.apply(
                    now_ms,
                    HostingCommand::Retry {
                        deployment: deployment.clone(),
                    },
                )
            }
            "ObserveListed" => hosted.apply(now_ms, HostingCommand::Observe),
            "ObserveTakenOver" => {
                // Another controller took the lease over at a strictly newer epoch and retagged
                // the resource; the listing carries the new tag.
                let epoch = hosted.leases.epoch(&deployment).saturating_add(1);
                hosted
                    .provider
                    .retag(&deployment, &ident(LIFECYCLE_SUCCESSOR)?, epoch);
                hosted.apply(now_ms, HostingCommand::Observe)
            }
            "ObserveUnlisted" => {
                // A partial listing that does not carry the resource.
                let mut partial =
                    FakeProvider::new(ident(LIFECYCLE_PROVIDER)?, ident(LIFECYCLE_ACCOUNT)?);
                partial.inventory_completeness(Completeness::Partial);
                hosted
                    .controller
                    .apply(now_ms, &mut partial, HostingCommand::Observe)
                    .map(|_| ())
            }
            "RequestStop" => hosted.apply(
                now_ms,
                HostingCommand::RequestStop {
                    deployment: deployment.clone(),
                },
            ),
            "ConfirmStopped" => hosted.apply(
                now_ms,
                HostingCommand::ConfirmStopped {
                    deployment: deployment.clone(),
                    evidence: ident(text(input, "evidence")?)?,
                },
            ),
            "Cancel" => hosted.apply(
                now_ms,
                HostingCommand::Cancel {
                    deployment: deployment.clone(),
                },
            ),
            other => return Err(format!("{other} is not a lifecycle command")),
        };
        if let Err(error) = applied {
            return Ok(refused(id, error));
        }
        let after = hosted.record(&deployment);
        let outcome = classify(command, before.as_ref(), after.as_ref(), now_ms);
        Ok(Answer {
            outcome,
            error: None,
            event: outcome
                .filter(|name| *name != "unchanged")
                .map(|_| PHASE_MOVED),
            id: id.to_owned(),
        })
    }

    fn declare(
        &mut self,
        id: &str,
        deployment: Identifier,
        input: &Value,
        now_ms: u64,
    ) -> Result<Answer, String> {
        let command = HostingCommand::Declare {
            spec: DeploymentSpec {
                id: deployment.clone(),
                provider: ident(LIFECYCLE_PROVIDER)?,
                account: ident(LIFECYCLE_ACCOUNT)?,
                model: ident("requested-model")?,
                image: ident("requested-image")?,
                resource_name: deployment.clone(),
                requested_lifetime_ms: LIFECYCLE_HOUR_MS,
            },
            request_id: ident(text(input, "request_id")?)?,
            authorization: ComputeAuthorization {
                ledger: ident(LIFECYCLE_LEDGER)?,
                reservation: ident(text(input, "reservation_id")?)?,
            },
        };
        if let Some(hosted) = self.records.get_mut(id) {
            // A second declaration goes to the controller that holds the first.
            return Ok(match hosted.apply(now_ms, command) {
                Ok(()) => Answer {
                    outcome: None,
                    error: None,
                    event: None,
                    id: id.to_owned(),
                },
                Err(error) => refused(id, error),
            });
        }
        let mut hosted = Hosted::empty()?;
        if let Err(error) = hosted.apply(now_ms, command) {
            return Ok(refused(id, error));
        }
        // The ownership lease every later mutation is fenced on, from a registry nobody else
        // holds a claim in.
        hosted
            .apply(now_ms, HostingCommand::Acquire { deployment })
            .map_err(|error| format!("a fresh lease registry refused the lease: {error}"))?;
        self.records.insert(id.to_owned(), hosted);
        Ok(Answer {
            outcome: Some("declared"),
            error: None,
            event: Some(RESOURCE_DECLARED),
            id: id.to_owned(),
        })
    }

    /// Every declared record with its phase as the lifecycle state.
    pub fn rows(&self) -> Vec<Value> {
        self.records
            .values()
            .flat_map(|hosted| hosted.controller.snapshot())
            .map(|record| {
                json!({
                    "id": record.deployment.as_str(),
                    "state": format!("{:?}", record.phase),
                    "request_id": record.request_id.as_str(),
                    "reservation_id": record.authorization.reservation.as_str(),
                    "stop_evidence": record.stop_evidence.as_ref().map(Identifier::as_str),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT, MAX_PROGRAM_BYTES, MAX_STEPS, run, transitions, within_size};
    use llm_provision::{
        Completeness, Dispatch, EvidenceKind, HostingError, Phase, ProviderState, StopReason,
    };
    use serde_json::{Value, json};

    const POLICY: &str = r#"{"controller":"controller-a","provider":"fake","account":"account-1","ledger":"scope","max_active":2,"max_lifetime_ms":10000,"lease_ms":1000}"#;

    fn program(steps: &str) -> Value {
        run(&format!(r#"{{"policy":{POLICY},"steps":[{steps}]}}"#))
    }

    // --- published bounds, at the literal number and one past it ------------------------------

    #[test]
    fn the_step_bound_runs_at_128_and_refuses_129() {
        assert_eq!(MAX_STEPS, 128);
        for (count, valid) in [(128, true), (129, false)] {
            let steps: Vec<String> = (0..count)
                .map(|index| format!(r#"{{"step":"tick","at_ms":{index}}}"#))
                .collect();
            let facts = program(&steps.join(","));
            assert_eq!(facts["valid_program"], json!(valid), "{count} steps");
            if valid {
                assert_eq!(
                    facts["results"].as_array().expect("results").len(),
                    count,
                    "every step ran"
                );
            } else {
                assert_eq!(facts["results"], json!([]));
                assert_eq!(facts["allocations"], Value::Null);
            }
        }
    }

    #[test]
    fn the_concurrency_bound_runs_at_32_and_refuses_33() {
        assert_eq!(MAX_CONCURRENT, 32);
        for (count, valid) in [(32, true), (33, false)] {
            let owners: Vec<String> = (0..count)
                .map(|index| format!(r#""owner-{index}""#))
                .collect();
            let steps = format!(
                r#"{{"step":"declare","at_ms":1,"spec":{{"id":"d1","provider":"fake","account":"account-1","model":"m","image":"i","resource_name":"pool-slot","requested_lifetime_ms":5000}},"request_id":"r1","authorization":{{"ledger":"scope","reservation":"res"}}}},{{"step":"concurrent-acquire","at_ms":1,"deployment":"d1","lease_ms":1000,"owners":[{}]}}"#,
                owners.join(",")
            );
            let facts = program(&steps);
            assert_eq!(facts["valid_program"], json!(valid), "{count} owners");
            if valid {
                assert_eq!(facts["concurrent_granted"], json!([1]));
                assert_eq!(
                    facts["concurrent_errors"].as_array().expect("errors").len(),
                    count - 1
                );
            }
        }
    }

    #[test]
    fn the_document_bound_admits_65536_bytes_and_refuses_65537() {
        // Literals, not `MAX_PROGRAM_BYTES ± 1`: a bound measured against its own constant
        // cannot notice the constant moving.
        assert_eq!(MAX_PROGRAM_BYTES, 65_536);
        assert!(within_size(&"x".repeat(65_536)));
        assert!(!within_size(&"x".repeat(65_537)));
        assert!(within_size(""));
    }

    // --- the observation surface reports absence as absence -------------------------------------

    #[test]
    fn a_program_that_cannot_run_reports_no_counts_at_all() {
        let facts = run("{not a program");
        for field in [
            "allocations",
            "submits",
            "stops",
            "lists",
            "totals",
            "now_ms",
        ] {
            assert_eq!(
                facts[field],
                Value::Null,
                "{field} must be absent, not zero"
            );
        }
        assert_eq!(facts["valid_program"], json!(false));
    }

    #[test]
    fn a_refused_policy_reports_the_counts_the_provider_really_has() {
        let refused = run(
            r#"{"policy":{"controller":"c","provider":"fake","account":"a","ledger":"l","max_active":0,"max_lifetime_ms":1,"lease_ms":1},"steps":[]}"#,
        );
        assert_eq!(refused["valid_program"], json!(true));
        assert_eq!(refused["error_code"], json!("invalid-policy"));
        for field in ["allocations", "submits", "stops", "lists"] {
            assert_eq!(
                refused[field],
                json!(0),
                "{field} is nought, and nought is known"
            );
        }
        assert_eq!(refused["totals"], Value::Null);
    }

    // --- the specification and the Rust inventories name the same values -------------------------

    const SPEC: &str = include_str!("../../../spec/domains/hosting.yaml");

    /// One declared variant: its name and its attributes, in declaration order.
    type Variant = (String, Vec<(String, String)>);

    /// The variants a domain declares for one enum type, read out of the specification, in
    /// either form ESS takes: `variants: [A, B]`, or one `- {name: A, attributes: {..}}` line
    /// per variant.
    fn declared_variants(type_name: &str) -> Vec<Variant> {
        let start = SPEC
            .find(&format!("  - name: {type_name}\n"))
            .unwrap_or_else(|| panic!("spec/domains/hosting.yaml declares no {type_name}"));
        let rest = &SPEC[start + 1..];
        let end = [
            rest.find("\n  - name: "),
            rest.find("\n  #"),
            rest.find("\nentities:"),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
        let block = &rest[..end];
        if let Some(inline) = block
            .lines()
            .find_map(|line| line.trim().strip_prefix("variants: ["))
        {
            return inline
                .trim_end_matches(']')
                .split(',')
                .map(|name| (name.trim().to_owned(), Vec::new()))
                .collect();
        }
        let (_, listed) = block
            .split_once("\n    variants:")
            .unwrap_or_else(|| panic!("{type_name} declares no variants"));
        listed
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- {name: "))
            .map(|line| match line.split_once(", attributes: {") {
                Some((name, attributes)) => (
                    name.to_owned(),
                    attribute_pairs(attributes.trim_end_matches('}')),
                ),
                None => (line.trim_end_matches('}').to_owned(), Vec::new()),
            })
            .collect()
    }

    /// `k: v, k: "v, with a comma"` as ordered pairs, quotes removed.
    fn attribute_pairs(mut text: &str) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        while let Some((key, after)) = text.split_once(':') {
            let after = after.trim_start();
            let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
                let close = quoted.find('"').expect("a closing quote");
                (&quoted[..close], &quoted[close + 1..])
            } else {
                let stop = after.find(',').unwrap_or(after.len());
                (&after[..stop], &after[stop..])
            };
            pairs.push((key.trim().to_owned(), value.trim().to_owned()));
            text = remainder.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        }
        pairs
    }

    /// The kebab-case codes of a declared enum, as the crate's `code()` spells them.
    fn declared(type_name: &str) -> Vec<String> {
        declared_variants(type_name)
            .into_iter()
            .map(|(name, _)| kebab(&name))
            .collect()
    }

    fn codes<T: Copy>(all: &[T], code: fn(T) -> &'static str) -> Vec<String> {
        all.iter().map(|value| code(*value).to_owned()).collect()
    }

    fn kebab(name: &str) -> String {
        let mut out = String::new();
        for (index, character) in name.char_indices() {
            if character.is_ascii_uppercase() && index != 0 {
                out.push('-');
            }
            out.push(character.to_ascii_lowercase());
        }
        out
    }

    /// The macro in `closed.rs` makes each inventory un-driftable inside Rust. It cannot see
    /// `spec/domains/hosting.yaml`, which re-declares the same vocabularies by hand, so this is
    /// the comparison that closes that gap: a variant added, renamed or removed on either side
    /// without the other fails here.
    #[test]
    fn every_vocabulary_is_declared_identically_in_rust_and_in_the_specification() {
        let pairs: [(&str, Vec<String>); 7] = [
            ("llm-gateway.hosting.Phase", codes(Phase::ALL, Phase::code)),
            (
                "llm-gateway.hosting.StopReason",
                codes(StopReason::ALL, StopReason::code),
            ),
            (
                "llm-gateway.hosting.Dispatch",
                codes(Dispatch::ALL, Dispatch::code),
            ),
            (
                "llm-gateway.hosting.ProviderState",
                codes(ProviderState::ALL, ProviderState::code),
            ),
            (
                "llm-gateway.hosting.HostingError",
                codes(HostingError::ALL, HostingError::code),
            ),
            (
                "llm-gateway.hosting.EvidenceKind",
                codes(EvidenceKind::ALL, EvidenceKind::code),
            ),
            (
                "llm-gateway.hosting.Completeness",
                codes(Completeness::ALL, Completeness::code),
            ),
        ];
        for (type_name, mut rust) in pairs {
            let mut spec = declared(type_name);
            rust.sort();
            spec.sort();
            assert_eq!(spec, rust, "{type_name}");
        }
    }

    /// Row 12: every refusal is declared with the code and the fixed message the crate
    /// publishes, so a reader of the specification is shown what a caller reads.
    #[test]
    fn every_hosting_refusal_is_declared_with_its_code_and_message() {
        let declared = declared_variants("llm-gateway.hosting.HostingError");
        let expected: Vec<Variant> = HostingError::ALL
            .iter()
            .map(|error| {
                (
                    format!("{error:?}"),
                    vec![
                        ("code".to_owned(), error.code().to_owned()),
                        ("message".to_owned(), error.to_string()),
                    ],
                )
            })
            .collect();
        assert_eq!(declared, expected);
    }

    /// Rows 9 and 10: the evidence routes and the two completeness words carry the codes the
    /// crate attaches and reads.
    #[test]
    fn every_evidence_kind_and_completeness_is_declared_with_its_code() {
        for (type_name, rust) in [
            (
                "llm-gateway.hosting.EvidenceKind",
                codes(EvidenceKind::ALL, EvidenceKind::code),
            ),
            (
                "llm-gateway.hosting.Completeness",
                codes(Completeness::ALL, Completeness::code),
            ),
        ] {
            let declared: Vec<String> = declared_variants(type_name)
                .into_iter()
                .map(|(name, attributes)| {
                    assert_eq!(
                        attributes.first().map(|(key, _)| key.as_str()),
                        Some("code"),
                        "{type_name}.{name}"
                    );
                    attributes[0].1.clone()
                })
                .collect();
            assert_eq!(declared, rust, "{type_name}");
        }
    }

    /// Row 13 and the slot rule: which phases owe a stop and which occupy a slot against the
    /// resource ceiling are declared as the crate decides them.
    #[test]
    fn every_phase_declares_whether_it_owes_a_stop_and_occupies_a_slot() {
        let declared = declared_variants("llm-gateway.hosting.Phase");
        let expected: Vec<Variant> = Phase::ALL
            .iter()
            .map(|phase| {
                (
                    format!("{phase:?}"),
                    vec![
                        ("owes_a_stop".to_owned(), phase.owes_a_stop().to_string()),
                        (
                            "occupies_a_slot".to_owned(),
                            phase.occupies_a_slot().to_string(),
                        ),
                    ],
                )
            })
            .collect();
        assert_eq!(declared, expected);
    }

    /// Row 11: the table is declared, edge by edge and in order, as `TRANSITIONS` holds it,
    /// so an edge removed from the library fails here as well as in the table scenario.
    #[test]
    fn the_declared_transition_table_is_the_published_constant() {
        let declared: Vec<(String, String)> = declared_variants("llm-gateway.hosting.Transition")
            .into_iter()
            .map(|(name, attributes)| {
                let value = |key: &str| {
                    attributes.iter().find(|(name, _)| name == key).map_or_else(
                        || panic!("{name} declares no {key}"),
                        |(_, value)| kebab(value),
                    )
                };
                (value("from"), value("to"))
            })
            .collect();
        let published: Vec<(String, String)> = llm_provision::TRANSITIONS
            .iter()
            .map(|(from, to)| (from.code().to_owned(), to.code().to_owned()))
            .collect();
        assert_eq!(declared, published);
    }

    /// The block an entity declares, from its `  - name:` line to the next declaration.
    fn entity_block(entity: &str) -> &'static str {
        let start = SPEC
            .find(&format!("  - name: {entity}\n"))
            .unwrap_or_else(|| panic!("spec/domains/hosting.yaml declares no {entity}"));
        let rest = &SPEC[start + 1..];
        let end = [
            rest.find("\n  - name: "),
            rest.find("\n  #"),
            rest.find("\ncommands:"),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(rest.len());
        &rest[..end]
    }

    /// `[A, B]` as its names, in order.
    fn listed(text: &str) -> Vec<String> {
        text.trim()
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or_else(|| panic!("`{text}` is not a flow list"))
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// One declared transition: its name, its `from` states and its `to` state.
    type Move = (String, Vec<String>, String);

    /// An entity's lifecycle as declared: `initial`, `states`, `terminal` and every transition,
    /// each read from the one flow-style line it takes. A line that does not read is a failure,
    /// so a change of layout cannot quietly empty the table this compares.
    fn declared_lifecycle(entity: &str) -> (String, Vec<String>, Vec<String>, Vec<Move>) {
        let block = entity_block(entity);
        let (_, body) = block
            .split_once("\n    lifecycle:\n")
            .unwrap_or_else(|| panic!("{entity} declares no lifecycle block"));
        let mut initial = None;
        let mut states = None;
        let mut terminal = None;
        let mut moves = Vec::new();
        for line in body.lines().take_while(|line| line.starts_with("      ")) {
            let line = line.trim();
            if let Some(value) = line.strip_prefix("initial: ") {
                initial = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("states: ") {
                states = Some(listed(value));
            } else if let Some(value) = line.strip_prefix("terminal: ") {
                terminal = Some(listed(value));
            } else if let Some(entry) = line.strip_prefix("- {name: ") {
                let (name, rest) = entry
                    .split_once(", from: ")
                    .unwrap_or_else(|| panic!("`{line}` names no from"));
                let (from, to) = rest
                    .split_once(", to: ")
                    .unwrap_or_else(|| panic!("`{line}` names no to"));
                let to = to
                    .strip_suffix('}')
                    .unwrap_or_else(|| panic!("`{line}` is not one flow mapping"));
                moves.push((name.to_owned(), listed(from), to.to_owned()));
            } else if line != "transitions:" {
                panic!("{entity}: unread lifecycle line `{line}`");
            }
        }
        (
            initial.unwrap_or_else(|| panic!("{entity} declares no initial state")),
            states.unwrap_or_else(|| panic!("{entity} declares no states")),
            terminal.unwrap_or_else(|| panic!("{entity} declares no terminal states")),
            moves,
        )
    }

    /// Acceptance 1 of the lifecycle: the ownership state machine is declared as the ESS
    /// lifecycle of `OwnedResource`, state by state and edge by edge as `Phase` and `TRANSITIONS`
    /// hold it. A pair added to or removed from either side fails here; so does a terminal phase
    /// that gains an edge.
    #[test]
    fn the_owned_resource_lifecycle_is_the_published_transition_table() {
        let (initial, states, terminal, moves) =
            declared_lifecycle("llm-gateway.hosting.OwnedResource");
        assert_eq!(initial, "Declared");
        let phases: Vec<String> = Phase::ALL
            .iter()
            .map(|phase| format!("{phase:?}"))
            .collect();
        assert_eq!(states, phases, "the states are the phases, in their order");
        let terminal: std::collections::BTreeSet<String> = terminal.into_iter().collect();
        let published_terminal: std::collections::BTreeSet<String> = Phase::ALL
            .iter()
            .filter(|phase| phase.terminal())
            .map(|phase| format!("{phase:?}"))
            .collect();
        assert_eq!(terminal, published_terminal);
        let declared: std::collections::BTreeSet<(String, String)> = moves
            .iter()
            .flat_map(|(_, from, to)| from.iter().map(|from| (from.clone(), to.clone())))
            .collect();
        let published: std::collections::BTreeSet<(String, String)> = llm_provision::TRANSITIONS
            .iter()
            .map(|(from, to)| (format!("{from:?}"), format!("{to:?}")))
            .collect();
        assert_eq!(declared, published);
        assert_eq!(published.len(), llm_provision::TRANSITIONS.len());
    }

    /// Acceptance 2, as far as the specification can say it: every declared transition is taken
    /// by at least one command outcome (`moves:`), so a synthesized scenario drives each one
    /// through the controller.
    #[test]
    fn every_owned_resource_transition_is_moved_by_a_command_outcome() {
        let (_, _, _, transitions) = declared_lifecycle("llm-gateway.hosting.OwnedResource");
        assert!(
            !transitions.is_empty(),
            "the lifecycle declares no transition"
        );
        for (name, _, _) in transitions {
            let taken = format!("moves: llm-gateway.hosting.OwnedResource.{name}\n");
            assert!(SPEC.contains(&taken), "no outcome moves `{name}`");
        }
    }

    #[test]
    fn the_published_transition_table_is_reported_as_the_library_holds_it() {
        let reported = transitions();
        let rows = reported.as_array().expect("an array");
        assert_eq!(rows.len(), llm_provision::TRANSITIONS.len());
        for (row, (from, to)) in rows.iter().zip(llm_provision::TRANSITIONS) {
            assert_eq!(row["from"], json!(from.code()));
            assert_eq!(row["to"], json!(to.code()));
        }
    }
}
