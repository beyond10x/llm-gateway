---
format: aep.planning-md/3
id: decision-blocker:runpod-control-plane
kind: decision-blocker
status: cleared
title: Where Runpod is described and how its API is reached is not decided (design choice D1)
relations:
- blocks: story:runpod-production-transport
- blocks: story:provider-key-files
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T10:03:41Z", actor: "human:timo", revision: 3}
---
## Question

Design choice D1 of `docs/design/runpod-clients.md`: where Runpod is described, how its API is
reached and where the keys live. A: a provider description in beyond10x/llm, the control plane
through beyond10x/connectors. B: an own client here with keys from beyond10x/secrets. C: an own
client with keys from trusted files, as `story:runpod-production-transport` and
`story:provider-key-files` are written today. Recommended: A.

## Decided 2026-10-08: A

The operator chose A. Runpod is a provider description in beyond10x/llm `crates/llm-providers`; its
control plane (create, list, terminate a pod) is reached through beyond10x/connectors with a Runpod
OpenAPI bundle; this gateway holds only each model's vLLM key. The Runpod description and the
OpenAPI bundle are requested from those two repositories.

## Cleared when

The choice is recorded (done above). `story:runpod-production-transport` and
`story:provider-key-files` are rewritten for A before either starts, and then depend on the llm and
connectors releases that carry the Runpod description and bundle.

## Rewritten for A

Both stories are rewritten for A: `story:provider-key-files` (implemented 2026-10-08, reads only each model's vLLM key) and `story:runpod-production-transport` (2026-10-08, control plane through connectors; still blocked by `upstream-blocker:connectors-runpod-bundle`).
