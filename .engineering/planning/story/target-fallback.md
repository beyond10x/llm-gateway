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
revision: 2
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

`crates/llm-gateway/src/relay.rs` (`RelayTargets`, `relay::serve`),
`crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs`,
`spec/domains/upstream.yaml`, `spec/domains/deployment.yaml`, `contracts/gateway/scenarios`,
`checks/conformance`, `docs/gateway.md`.
