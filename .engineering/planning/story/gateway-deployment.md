---
format: aep.planning-md/3
id: story:gateway-deployment
kind: story
status: active
title: The binary relays model calls to Runpod pods with the model's vLLM key
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:provider-key-files
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: inferred
  path: spec/domains/gateway.yaml
revision: 15
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 14, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"review_outcome":2}}}
---
## Outcome

The `b10x-llm-gateway` binary relays model calls. It composes the gateway with `bind_with_relay`
and a `RelayTargets` implementation over `RunpodPool`, and the relay sends each pod the model's
vLLM key as `authorization: Bearer <key>` (row B8).

## Rows

B8, from `docs/llmgw-capability-matrix.md`. On 2026-10-06 rows K7 and B9 moved to
`story:provider-key-files`, rows R1 and R5 to `story:public-model-listing`, and rows D1 and D3 to
`story:container-image`. The `story-scoper` found the four slices independent.

## ESS first

`spec/domains/gateway.yaml` declares the bearer the relay adds to the outbound head. The
`Relay` program grammar gains what a scenario needs to observe it.

## Acceptance

1. `spec/domains/gateway.yaml` declares the bearer the relay adds to the outbound head. A `Relay`
   scenario observes it at a fixture pod, and the observation has a field for the authorization
   the pod received.
2. A test composes the binary's `serve` with `EmulatedRunpod` through a test seam in
   `crates/llm-gateway-cli`, together with a loopback pod. The shipped binary has no option that
   selects the emulator. A chat request reaches the pod with the model's vLLM key, from
   `story:provider-key-files`, as the bearer, and its answer reaches the client.
3. The owner's credential never reaches the pod. A test searches every byte the pod receives.
4. The inputs `RunpodPool` needs are a `ComputeAuthorization`, a wall clock, a `LeaseRegistry` and
   a `HostingPolicy`. Each comes from the deployment document or is a fixed value, and the
   specification and `docs/hosting.md` name which. The story also decides what
   `idle_timeout_minutes = 0` means, which the matrix leaves to whichever story composes the pool
   (row K27), and records the answer in `deployment.yaml` with a test.
5. Row B8 is `covered` in the matrix. Composing the production transport is
   `story:live-runpod-wiring`.

## Depends on

`story:provider-key-files`, which supplies the vLLM key.

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/src/lib.rs`, a new relay-target
module under `crates/llm-gateway-cli/src/`, `crates/llm-gateway-cli/Cargo.toml`,
`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/tests/wire_relay.rs`,
`crates/llm-gateway-cli/tests/binary.rs`, `checks/conformance/src/gateway.rs`,
`spec/domains/gateway.yaml`.
