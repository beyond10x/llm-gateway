---
format: aep.planning-md/3
id: story:live-runpod-wiring
kind: story
status: draft
title: The binary starts real pods through the production Runpod transport
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
- depends_on: story:runpod-production-transport
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
revision: 3
---
## Outcome

The `b10x-llm-gateway` binary composes `RunpodProvider` over the production Runpod transport. It
hands the transport the Runpod API key and each model's vLLM key, both read by
`story:provider-key-files`: the transport needs the first for the API and the second for the
readiness probe (row B6). A deployment that names `runpod_api_key_file` then starts real pods.
The shipped binary never composes `EmulatedRunpod`. Without the key file it starts no pod and
answers `target-unavailable`, following llm's rule that "a provider adapter never silently
changes endpoints or billing accounts" (beyond10x/llm `AGENTS.md:18`).

## Why

The replan of 2026-10-06 found that no story wires the production transport into the binary.
`story:gateway-deployment` closes row B8 through a test seam over `EmulatedRunpod`.
`story:runpod-production-transport` builds the transport in `crates/llm-runpod`, and its
acceptance keeps it off the binary. Without this story, llm `story:llmgw-retirement` has no
binary that reaches a real pod.

## ESS first

`spec/domains/deployment.yaml` declares the provider's `api_base_url` key (default: the Runpod
API) before any code. It is the only way a test points the binary at a fixture.

## Acceptance

1. A process test sets `api_base_url` to a loopback fixture of the Runpod API and runs the binary.
   A request for a cold model makes the fixture receive one create with the declared GPU types.
   The fixture's pod then receives the readiness probe with the model's vLLM key, and after that
   the relayed request.
2. The Runpod key reaches the fixture as the API's authorization. Neither key appears in any
   response or on any line of standard error.
3. A document without `runpod_api_key_file` starts. A request for a Runpod-served model answers
   `target-unavailable`, and the fixture receives nothing. A test checks this, and it also checks
   that the binary has no option that selects the emulator.
4. No test makes a paid call. A live run against Runpod is qualification evidence for llm
   `story:llmgw-retirement` and is recorded there, outside the gate.

## Depends on

`story:gateway-deployment` (the pool-backed relay and, through it, `story:provider-key-files`'s
keys) and `story:runpod-production-transport` (the transport).

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/src/config.rs`,
`crates/llm-gateway-cli/Cargo.toml`, `crates/llm-gateway-cli/tests/binary.rs`,
`spec/domains/deployment.yaml`, `docs/hosting.md`.
