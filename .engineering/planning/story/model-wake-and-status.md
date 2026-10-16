---
format: aep.planning-md/3
id: story:model-wake-and-status
kind: story
status: active
title: The owner can wake a model and read its start phase without a model call
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
- depends_on: story:spend-caps
- depends_on: story:model-profile-fallback
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: inferred
  path: crates/llm-gateway/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/hosting.md
- confidence: inferred
  path: spec/domains/gateway.yaml
revision: 15
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T20:21:01Z", actor: "human:timo", revision: 14}
- {from: "proposed", to: "active", at: "2026-10-16T20:21:01Z", actor: "human:timo", revision: 15}
---
## Outcome

The owner can start a relayed model ahead of demand and read where its start is, without sending
a model call.

## Acceptance

1. `POST` on a per-model wake route (owner credential) starts the model's pod as a held request
   would, answers `202` at once, and is idempotent while a start is in progress or the pod is ready.
2. `GET` on a per-model status route (owner credential) answers the phase (`stopped`, `starting`
   with its sub-phase where the provider reports one, `ready`, `stopping`) and the profile, from
   the hosting state machine; it starts nothing.
3. Both routes are in `docs/gateway.md`'s route table and refusal reference, the specification and
   conformance scenarios; a wake counts against the spend check of `story:spend-caps`.

## Blocked

Not blocked.

The on-demand GPU lifecycle is decided (`architecture-decision-record:on-demand-gpu-lifecycle`,
accepted; `decision-blocker:on-demand-gpu-lifecycle` cleared). Delivered offline against
`EmulatedRunpod` and the `connectors-fixture` binary; no test reaches a provider API.
