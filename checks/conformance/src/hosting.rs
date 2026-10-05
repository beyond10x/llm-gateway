//! Owned-resource hosting conformance observations.
//!
//! This module translates an authored fixture program into calls on the real
//! `b10x-llm-provision` public API and reports what those calls returned. It reimplements no
//! lifecycle rule, reads no suite, and branches on no scenario name: every phase, refusal,
//! counter and obligation below comes back out of the library.

use std::sync::Arc;

use ess_conformance::target::TargetError;
use llm_provision::{
    AmbiguousCreate, Completeness, ComputeAuthorization, Controller, DeploymentRecord,
    DeploymentSpec, Dispatch, FakeProvider, HostingCommand, HostingError, HostingPolicy,
    Idempotency, Identifier, LeaseRegistry, Phase, TRANSITIONS,
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

#[cfg(test)]
mod tests {
    use super::{MAX_CONCURRENT, MAX_PROGRAM_BYTES, MAX_STEPS, run, transitions, within_size};
    use llm_provision::{Dispatch, Phase, ProviderState, StopReason};
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

    /// The variant list a domain declares for one type, read out of the specification.
    fn declared(type_name: &str) -> Vec<String> {
        let source = include_str!("../../../spec/domains/hosting.yaml");
        let mut lines = source
            .lines()
            .skip_while(|line| line.trim() != format!("- name: {type_name}"));
        lines.next().expect("the type is declared");
        let variants = lines
            .find_map(|line| {
                line.trim()
                    .strip_prefix("variants: [")
                    .map(ToOwned::to_owned)
            })
            .expect("the type declares variants");
        variants
            .trim_end_matches(']')
            .split(',')
            .map(|name| kebab(name.trim()))
            .collect()
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
    /// `spec/domains/hosting.yaml`, which re-declares the same four vocabularies by hand, so
    /// this is the comparison that closes that gap: a variant added, renamed or removed on
    /// either side without the other fails here.
    #[test]
    fn every_vocabulary_is_declared_identically_in_rust_and_in_the_specification() {
        let pairs: [(&str, Vec<String>); 4] = [
            (
                "llm-gateway.hosting.Phase",
                Phase::ALL
                    .iter()
                    .map(|value| value.code().to_owned())
                    .collect(),
            ),
            (
                "llm-gateway.hosting.StopReason",
                StopReason::ALL
                    .iter()
                    .map(|value| value.code().to_owned())
                    .collect(),
            ),
            (
                "llm-gateway.hosting.Dispatch",
                Dispatch::ALL
                    .iter()
                    .map(|value| value.code().to_owned())
                    .collect(),
            ),
            (
                "llm-gateway.hosting.ProviderState",
                ProviderState::ALL
                    .iter()
                    .map(|value| value.code().to_owned())
                    .collect(),
            ),
        ];
        for (type_name, mut rust) in pairs {
            let mut spec = declared(type_name);
            rust.sort();
            spec.sort();
            assert_eq!(spec, rust, "{type_name}");
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
