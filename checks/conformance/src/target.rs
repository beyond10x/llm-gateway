use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

use ess_conformance::target::{
    ConformanceTarget, EventObservationRequest, ExternalOutcomeControl, ImplementationIdentity,
    ObservedEvent, RedeliveryRequest, ScenarioContext, SemanticCommandRequest,
    SemanticCommandResult, SemanticViewRequest, SemanticViewResult, TargetError,
};
use ess_primitives::{consistency::ConsistencyToken, node::Node};
use serde_json::Value;

/// Only returned production facts are retained. This target never reads the suite,
/// its expected assertions, or a scenario name to determine an answer.
pub struct ServingTarget {
    version: String,
    observation: RefCell<Option<BTreeMap<String, Node>>>,
    token: RefCell<Option<ConsistencyToken>>,
    sequence: Cell<u64>,
    observed_view: RefCell<Option<String>>,
}
impl ServingTarget {
    pub const fn new(version: String) -> Self {
        Self {
            version,
            observation: RefCell::new(None),
            token: RefCell::new(None),
            sequence: Cell::new(0),
            observed_view: RefCell::new(None),
        }
    }
}

/// What a domain module observed: the facts its public library returned, the view that
/// answers for them, the event the command emits, and the fact whose truth that event carries.
///
/// A domain module owns its own file and returns this; `execute_command` below is shared and
/// stays out of every unit's assignment.
pub struct Observed {
    pub facts: Value,
    pub view: &'static str,
    pub event: &'static str,
    pub field: &'static str,
}

fn unavailable(error: impl std::fmt::Display) -> TargetError {
    TargetError::unavailable("serving observation", error.to_string())
}
fn unsupported(operation: &str) -> TargetError {
    TargetError::unsupported(
        operation,
        "library observation adapter exposes no such operation",
    )
}

impl ConformanceTarget for ServingTarget {
    fn identity(&self) -> Result<ImplementationIdentity, TargetError> {
        Ok(ImplementationIdentity::new(
            "llm-gateway-serving-libraries",
            &self.version,
        ))
    }
    fn begin_scenario(&self, _: &ScenarioContext) -> Result<(), TargetError> {
        self.observation.replace(None);
        self.token.replace(None);
        self.observed_view.replace(None);
        Ok(())
    }
    fn end_scenario(&self, _: &ScenarioContext) -> Result<(), TargetError> {
        self.observation.replace(None);
        self.token.replace(None);
        self.observed_view.replace(None);
        Ok(())
    }
    fn execute_command(
        &self,
        request: SemanticCommandRequest,
    ) -> Result<SemanticCommandResult, TargetError> {
        let input = serde_json::to_value(request.input).map_err(unavailable)?;
        let command = request.command.to_string();
        // Each domain module answers only its own commands and returns `None` otherwise,
        // so a new domain is a new file rather than an edit to this shared one.
        let observed = crate::hosting::observe(&command, &input)
            .or_else(|| crate::runpod::observe(&command, &input))
            .or_else(|| crate::gateway::observe(&command, &input))
            .ok_or_else(|| unsupported(&command))??;
        let Observed {
            facts,
            view,
            event,
            field,
        } = observed;
        let notification = facts[field]
            .as_bool()
            .ok_or_else(|| unavailable("missing notification fact"))?;
        self.observed_view.replace(Some(view.to_owned()));
        self.observation
            .replace(Some(serde_json::from_value(facts).map_err(unavailable)?));
        // This local adapter notification says observation finished. The actual
        // result is asserted independently through the domain's view.
        let mut result = SemanticCommandResult::took(ess_conformance::scenario::OutcomeRef::new(
            request.command,
            "observed".parse().map_err(unavailable)?,
        ));
        let sequence = self
            .sequence
            .get()
            .checked_add(1)
            .ok_or_else(|| unavailable("sequence exhausted"))?;
        self.sequence.set(sequence);
        let token = ConsistencyToken::new(format!("{}:{sequence}", request.correlation))
            .map_err(unavailable)?;
        self.token.replace(Some(token.clone()));
        result.consistency = Some(token);
        result.direct_events.push(
            ObservedEvent::new(event.parse().map_err(unavailable)?)
                .with(field, Node::Bool(notification)),
        );
        Ok(result)
    }
    fn query_view(&self, request: SemanticViewRequest) -> Result<SemanticViewResult, TargetError> {
        let view = request.view.to_string();
        let known = [
            crate::hosting::VIEWS,
            crate::runpod::VIEWS,
            crate::gateway::VIEWS,
        ]
        .iter()
        .any(|views| views.contains(&view.as_str()));
        if !known || !request.params.is_empty() {
            return Err(unsupported(&view));
        }
        if self.observed_view.borrow().as_ref() != Some(&view) {
            return Ok(SemanticViewResult::of(std::iter::empty()));
        }
        if request
            .consistency
            .token()
            .is_some_and(|token| Some(token) != self.token.borrow().as_ref())
        {
            return Err(unavailable(
                "requested observation is not retained in this scenario",
            ));
        }
        Ok(SemanticViewResult::of(
            self.observation.borrow().iter().cloned(),
        ))
    }
    fn observe_events(
        &self,
        _: EventObservationRequest,
    ) -> Result<Vec<ObservedEvent>, TargetError> {
        Err(unsupported("asynchronous events"))
    }
    fn configure_external_outcome(&self, _: ExternalOutcomeControl) -> Result<(), TargetError> {
        Err(unsupported("forcing an outcome"))
    }
    fn redeliver_event(&self, _: RedeliveryRequest) -> Result<(), TargetError> {
        Err(unsupported("redelivering an event"))
    }
}
