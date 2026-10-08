---
format: aep.planning-md/3
id: story:live-runpod-wiring
kind: story
status: active
title: The binary starts real pods through the production Runpod transport
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
- depends_on: story:runpod-production-transport
- depends_on: story:model-tool-calling
scope:
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: inferred
  path: docs/hosting.md
- confidence: inferred
  path: spec/domains/deployment.yaml
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T18:12:15Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-08T18:12:15Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":4}}}
---
## Outcome

The `b10x-llm-gateway` binary composes `RunpodProvider` over the production Runpod transport.
Rewritten 2026-10-08 for design choice D1 = A: the Runpod authorization is the connectors
connection's, held in the connectors keyring, and the binary never holds the Runpod API key
(`runpod_api_key_file` stays refused, `story:provider-key-files` acceptance 5). The binary hands
the transport each model's vLLM key for the readiness probe (row B6), and reaches the control
plane through the connectors owner process that runs beside it (`docs/design/runpod-clients.md`
§ 4, "Work outside this repository"). The shipped binary never composes `EmulatedRunpod`. Without
a reachable connectors connection it starts no pod and answers `target-unavailable`, following
llm's rule that "a provider adapter never silently changes endpoints or billing accounts"
(beyond10x/llm `AGENTS.md:18`).

## Why

The replan of 2026-10-06 found that no story wires the production transport into the binary.
`story:gateway-deployment` closes row B8 through a test seam over `EmulatedRunpod`.
`story:runpod-production-transport` builds the transport in `crates/llm-runpod`, and its
acceptance keeps it off the binary. Without this story, llm `story:llmgw-retirement` has no
binary that reaches a real pod.

## ESS first

`spec/domains/deployment.yaml` declares the provider's connectors connection key (how the binary
names the connectors connection and reaches its owner process) before any code. It is the only
way a test points the binary at a fixture.

## Acceptance

1. A process test points the binary at a fixture of the connectors interface. A request for a
   cold model makes the fixture receive one `CreatePod` invocation with the declared GPU types.
   The fixture's pod then receives the readiness probe with the model's vLLM key, and after that
   the relayed request.
2. No key appears in any response or on any line of standard error.
3. Without a reachable connectors connection the binary starts, and a request for a
   Runpod-served model answers `target-unavailable` while the fixture receives nothing. A test
   checks this, and it also checks that the binary has no option that selects the emulator.
4. No test makes a paid call. A live run against Runpod is qualification evidence, recorded by
   `story:client-qualification`, outside the gate.

## Depends on

`story:gateway-deployment` (the pool-backed relay and, through it, `story:provider-key-files`'s
keys) and `story:runpod-production-transport` (the transport).

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/src/config.rs`,
`crates/llm-gateway-cli/Cargo.toml`, `crates/llm-gateway-cli/tests/binary.rs`,
`spec/domains/deployment.yaml`, `docs/hosting.md`.

## Order changed 2026-10-08

Depends on `story:model-tool-calling` as well: both change `spec/domains/deployment.yaml` and
`crates/llm-gateway-cli/src/serve.rs`, and this story lands after it.
