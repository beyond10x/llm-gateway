---
format: aep.planning-md/3
id: story:model-profile-fallback
kind: story
status: implemented
title: A model starts on its next declared profile when the provider has no capacity for the first
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-runpod/src/config.rs
- confidence: cited
  path: crates/llm-runpod/src/emulated.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: cited
  path: crates/llm-runpod/src/provider.rs
- confidence: cited
  path: crates/llm-runpod/src/request.rs
- confidence: cited
  path: crates/llm-runpod/src/transport.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: docs/hosting.md
- confidence: inferred
  path: docs/model-profiles.md
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/telemetry.yaml
revision: 20
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 18}
- {from: "proposed", to: "active", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 19}
- {from: "active", to: "implemented", at: "2026-10-17T01:21:02Z", actor: "human:timo", revision: 20, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

A relayed model can declare an ordered list of profiles (GPU type with the vLLM settings that fit
it); when the provider refuses a start for capacity, the next profile is tried, so a model starts
when its first choice is out of stock.

## Acceptance

1. `[models.<alias>]` accepts an ordered `profiles` list, each a GPU type list plus its vLLM
   settings (context length at least); the existing single declaration is the one-profile case.
2. A create the emulated provider refuses for capacity is retried with the next profile within the
   same startup deadline and spend check; any other refusal, and a create whose answer was lost, is
   not retried. The provider's create answer tells a capacity refusal from any other. A test drives
   each.
3. The usage record names the profile that served. The per-model status route of
   `story:model-wake-and-status` names it too; that half lands with that story.

## Blocked

Not blocked.

The on-demand GPU lifecycle is decided (`architecture-decision-record:on-demand-gpu-lifecycle`,
accepted; `decision-blocker:on-demand-gpu-lifecycle` cleared). Delivered offline against
`EmulatedRunpod` and the `connectors-fixture` binary; no test reaches a provider API.
