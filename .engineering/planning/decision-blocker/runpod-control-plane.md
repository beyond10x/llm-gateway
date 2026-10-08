---
format: aep.planning-md/3
id: decision-blocker:runpod-control-plane
kind: decision-blocker
status: open
title: Where Runpod is described and how its API is reached is not decided (design choice D1)
relations:
- blocks: story:runpod-production-transport
- blocks: story:provider-key-files
revision: 1
---
## Question

Design choice D1 of `docs/design/runpod-clients.md`: where Runpod is described, how its API is
reached and where the keys live. A: a provider description in beyond10x/llm, the control plane
through beyond10x/connectors. B: an own client here with keys from beyond10x/secrets. C: an own
client with keys from trusted files, as `story:runpod-production-transport` and
`story:provider-key-files` are written today. Recommended: A.

## Cleared when

The choice is recorded. With A, the two blocked stories are rewritten before they start and
depend on the llm and connectors releases that carry the Runpod description and bundle.
