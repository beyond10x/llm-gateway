---
format: aep.planning-md/3
id: story:hosting-lifecycle-declared
kind: story
status: draft
title: The ownership state machine is an ESS lifecycle that mutation can audit
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
- depends_on: story:hosting-spec-declarations
- depends_on: story:spec-diff-gate
- depends_on: story:runpod-production-transport
- depends_on: story:gateway-spec-declarations
scope:
- confidence: inferred
  path: checks/conformance/src/hosting.rs
- confidence: inferred
  path: checks/conformance/src/target.rs
- confidence: inferred
  path: spec/domains/hosting.yaml
revision: 5
---
## Outcome

The ownership state machine of an owned resource (`Phase` and `TRANSITIONS` in
`crates/llm-provision/src/machine.rs`) is an ESS lifecycle in `spec/domains/hosting.yaml`. Its
transitions are caused by declared command outcomes, so `ess verify conform mutate` and guard
analysis have rules to work on.

## Why

On 2026-10-06 the mutation audit emitted 4 mutants, and all 4 were stillborn. The IR declares 0
transitions, so a dropped or retargeted edge in the specification cannot be caught
(`docs/verification/spec-hardening.md`). The state machine is what bounds Runpod spend, so it is
the hosting rule most worth making mutable.

## Acceptance

1. `llm-gateway.hosting.OwnedResource` has the lifecycle `Declared`, `Requested`, `Active`,
   `Uncertain`, `StopRequired`, `Stopped`, `Cancelled`, `Disowned`. Its transitions are exactly
   the pairs in `TRANSITIONS`, and `Stopped`, `Cancelled` and `Disowned` are terminal.
2. Each transition is taken by a command outcome that the conformance target executes through the
   controller's public calls.
3. `ess verify conform mutate` with the `from-drop` and `transition-to` classes scores at least one
   mutant per transition. None survives, and none is inconclusive.
4. The existing 179 scenarios, and those that `story:hosting-spec-declarations` adds, still pass.

## Depends on

- `story:hosting-spec-declarations`. Both stories change `spec/domains/hosting.yaml` and
  `checks/conformance/src/hosting.rs`.
- `story:spec-diff-gate`. This story changes `spec/`, so it lands after the gate it must pass.
- `story:runpod-production-transport` and `story:gateway-spec-declarations`. Acceptance 2 needs
  an outcome per transition, and `checks/conformance/src/target.rs:104` names every command's
  outcome `observed`. Changing that, or `Observed`, touches every domain module that builds it:
  `checks/conformance/src/runpod.rs:59` (the transport story) and
  `checks/conformance/src/gateway.rs:62` (the declarations story).

## Scope (inferred)

`spec/domains/hosting.yaml`, `checks/conformance/src/hosting.rs`, `checks/conformance/src/target.rs`,
new scenarios under `contracts/hosting/scenarios/`.
