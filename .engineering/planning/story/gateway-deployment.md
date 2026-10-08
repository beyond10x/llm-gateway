---
format: aep.planning-md/3
id: story:gateway-deployment
kind: story
status: implemented
title: The binary relays model calls to Runpod pods with the model's vLLM key
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:provider-key-files
scope:
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/relaying.rs
- confidence: cited
  path: crates/llm-gateway/src/auth.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: crates/llm-runpod/src/lib.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: cited
  path: crates/llm-runpod/tests/runpod.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/hosting.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
revision: 41
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 14, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "active", to: "implemented", at: "2026-10-08T15:20:17Z", actor: "human:timo", revision: 41, decided_on: {"recorded":{"test_result":1,"review_outcome":4,"verification":1}}}
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

## Scope (confirmed)

Confirmed by the implementor of wave 2026-10-08-w03; the typed `scope` entries are this list.

- confirmed: `checks/conformance/src/gateway.rs`, `crates/llm-gateway-cli/src/lib.rs`,
  `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/tests/wire_relay.rs`,
  `spec/domains/gateway.yaml`, and the new module (`crates/llm-gateway-cli/src/relaying.rs`).
- wrong: `crates/llm-gateway-cli/src/serve.rs` holds only helpers at its end
  (`tests/adversary_w02.rs` pins its line numbers); the composition is `relaying.rs`.
- wrong: `crates/llm-gateway-cli/Cargo.toml` is unchanged; `b10x-llm-runpod` re-exports the
  hosting types instead.
- wrong: `crates/llm-gateway-cli/tests/binary.rs`; the tests are `tests/relaying.rs`.
- not listed, touched: `crates/llm-gateway/src/auth.rs` (`TargetBearer`), `crates/llm-gateway/src/lib.rs`,
  `crates/llm-runpod/src/pool.rs` (`invalidate`, `ensure_running`, the idle clock),
  `crates/llm-runpod/src/lib.rs`, `crates/llm-runpod/tests/runpod.rs`, `docs/gateway.md`,
  `docs/hosting.md`, `spec/domains/deployment.yaml`.
