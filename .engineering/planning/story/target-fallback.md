---
format: aep.planning-md/3
id: story:target-fallback
kind: story
status: draft
title: A route falls back to its next target before any answer byte reaches the client
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:hosted-endpoints
scope:
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/adversary_relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
revision: 16
---
## Outcome

A model in the deployment document can declare an ordered list of targets and turn fallback on.
For one request to such a model, the gateway tries the targets in position order. An attempt that
fails before any byte of its answer reaches the client moves the request to the next position,
with the model name rewritten to that target's own upstream name. Once an attempt's answer starts
reaching the client, that attempt is final.

## Why

All seven gateways in the 2026-10-06 research name provider fallback (`epic:gateway-features`).
The route inventory already has ordered targets and a `fallback_enabled` flag
(`crates/llm-gateway/src/inventory.rs:126-138`), but the binary sets the flag to false and numbers
positions by wire rather than by alternative (`crates/llm-gateway-cli/src/serve.rs:91`, `:112`).
A failed target ends the request `upstream-failed` (row W7).

## ESS first

`llm-gateway.upstream.TargetAttempt` and `AttemptFailure` in `spec/domains/upstream.yaml` are
marked `PLANNED`. The 429 rule and the `request_sent` record are decided there (`DECIDED
2026-10-06`). This story adds the observation and scenarios, and settles the marker `UNMAPPED:
how the deployment document declares a model's ordered targets and its fallback flag` before any
code.

## Acceptance

Each case is a `Relay` conformance scenario with loopback fixture targets at positions 0 and 1,
on a model that declares both and has fallback on. Its observation lists one `TargetAttempt` per
position and the target source's `invalidate` calls.

1. Position 0 refuses the connection. The client receives position 1's answer. The attempts are
   `Failed` (`Unreachable`, `request_sent` false) and `Relayed`. The target source receives
   `invalidate` for position 0 only.
2. Position 0 answers 503. The client receives position 1's answer. Position 0's attempt is
   `Failed` (`Status503`, `request_sent` true), and it is invalidated.
3. Position 0 answers 429. The client receives position 1's answer. Position 0's attempt is
   `Failed` (`RateLimited`), and it is **not** invalidated.
4. Position 0 sends part of its answer, then closes. The client receives what was sent, as today.
   Position 0's attempt has `output_visible` true, and position 1 is `NotTried` and never
   contacted.
5. With fallback off, position 0 failing behaves as today: `upstream-failed` (502), the existing
   W7 scenarios unchanged, and position 1 `NotTried`.
6. Both positions fail with 503. The client receives one `upstream-failed` (502).
7. Position 1 answers 429 after position 0 failed. The client receives that 429 unchanged.
8. Position 1 receives the same body bytes as position 0, except that `model` holds position 1's
   own upstream name. The test checks that position 0 received position 0's upstream name.

## Depends on

`story:hosted-endpoints`. That story adds the second kind of target, so a deployed model can list
a pod and a hosted endpoint. This story changes the `RelayTargets` port
(`crates/llm-gateway/src/relay.rs:86-94`) from one target per alias to an ordered list. That
port is also changed by `story:cold-start-hold` and implemented by `story:gateway-deployment`,
which are ordered before it through `story:hosted-endpoints`.

## Boundary

The gateway crate cannot take llm-routing's fallback (`crates/llm-gateway/tests/dependency_boundary.rs`).
The rule it implements is llm's, cited in `spec/domains/upstream.yaml`.

## Scope (inferred)

Superseded by `## Scope`, which `story-scoper` derived on 2026-10-06. The typed entries are in the
frontmatter `scope`.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-gateway/src/relay.rs`: the `RelayTargets` port (`:86-94`), `RelayModel`'s single `upstream_model` (`:97-130`), and the acquire/send/head/502-504 path in `relay::serve` (`:573-608`) — cited
- **Files:** `crates/llm-gateway-cli/src/serve.rs` at `:91` (positions numbered by wire) and `:112` (`fallback_enabled` hard-coded `false`) — cited
- **Files:** `crates/llm-gateway-cli/src/config.rs`: the outcome needs new keys in the closed document, and `ModelDocument` (`:64-65`) is `deny_unknown_fields` — cited
- **Files:** `spec/domains/upstream.yaml` (`:86-108`, `TargetAttempt` and the UNMAPPED marker this story settles) — cited
- **Files:** `spec/domains/deployment.yaml`: the marker names the deployment document, which is specified as `ModelDeclaration` (`:115`) and `DeploymentConfiguration` (`:142`) — cited
- **Files:** `checks/conformance/src/gateway.rs`: the `Relay` command driver (`:468`) and the only non-test `RelayTargets` implementor (`:801`) — cited
- **Files:** `contracts/ess-inputs.yaml`: every new scenario must be listed (`every_authored_scenario_is_declared`, `checks/conformance/src/gate.rs`) — cited
- **Files:** `contracts/suite.json`: regenerated whenever `spec/` or the scenarios change (AGENTS.md "Generated files") — cited
- **New files only:** 8 new `Relay` scenarios under `contracts/gateway/scenarios/`, names not yet known; the 3 existing `w7-*.yaml` stay as they are (acceptance 5) — cited
- **Also likely:** `spec/domains/gateway.yaml`: the `Relay` program has one `upstream_model` per model and one pod sequence (`:206-208`), and `RelayObservation` (`:160-188`) has no per-position field, so acceptance 8 and the attempt list probably land here — inferred
- **Also likely:** `crates/llm-gateway/tests/wire_relay.rs`, `crates/llm-gateway/tests/adversary_relay.rs`, `crates/llm-gateway/tests/gateway.rs`: they implement `RelayTargets` (`:368`, `:170`, `:491`) and must change if the port's signature does — inferred
- **Also likely:** `crates/llm-gateway-cli/tests/config.rs`: new document keys get their own config tests — inferred
- **Also likely:** `contracts/baseline.json`: the floors equal today's count (179); raising them is the convention, nothing enforces it — inferred
- **Also likely:** `contracts/schema/schema/types/llm-gateway.deployment.ModelDeclaration.schema.json` and `contracts/schema/schema/entities/llm-gateway.gateway.RelayObservation.schema.json`, regenerated if those spec types change — inferred
- **Documents:** `docs/gateway.md` "The relay", steps 7-8 (`:150-158`), which say a failed target answers `upstream-failed` — inferred
- **Confidence:** medium. The story names `relay.rs`, `serve.rs` and `upstream.yaml` by line, but the document and fixture-program shape is the UNMAPPED decision this story makes — inferred
- **Would collide with:** any unit touching `relay::serve` (`relay.rs:537-620`), the `RelayTargets` port or any of its implementors, the closed deployment document (`config.rs` `ModelDocument`, `deployment.yaml` `ModelDeclaration`), `serve.rs` `inventory()`, or any regeneration of `contracts/suite.json` or `contracts/schema/` — cited
- **Safety fact:** every `upstream-failed` return in `relay::serve` (`relay.rs:581`, `:594`, `:602`, `:607`) comes before `stream_answer` (`:609`), whose first client write is `:647`. The only earlier client write is the `100 Continue` interim (`:546`). So a fallback loop over `:573-608` cannot retry after an answer byte has reached the client. Also, `body` is dropped at `:571` and `rewritten` at `:592`, so a retry with another upstream name must keep the body. Step 2, unproven — inferred

Not established: the document shape (ordered targets under `[models.<alias>]` or a section of its own); whether the port change breaks all four `RelayTargets` implementors or adds a defaulted method; whether the attempt list extends `RelayObservation` or becomes a new `upstream.yaml` entity. `serve.rs` lines will shift when story:gateway-deployment wires the relay.
