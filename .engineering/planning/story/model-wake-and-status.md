---
format: aep.planning-md/3
id: story:model-wake-and-status
kind: story
status: draft
title: The owner can wake a model and read its start phase without a model call
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
- depends_on: story:spend-caps
revision: 1
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

By `decision-blocker:on-demand-gpu-lifecycle`.
