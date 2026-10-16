---
format: aep.planning-md/3
id: decision-blocker:on-demand-gpu-lifecycle
kind: decision-blocker
status: cleared
title: Is the on-demand GPU lifecycle, spend ceilings included, the gateway's?
relations:
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
- blocks: story:spend-caps
- blocks: story:model-profile-fallback
- blocks: story:model-wake-and-status
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-15T20:22:58Z", actor: "human:timo", revision: 3}
---
## Question

Does the gateway own the on-demand GPU lifecycle of its models, spend ceilings included, as
`architecture-decision-record:on-demand-gpu-lifecycle` proposes?

## Options

- A: accept the record; the governed stories can be accepted and planned into waves.
- B: keep the lifecycle outside the gateway; the governed stories are archived.
- C: accept spend ceilings only; `story:spend-caps` proceeds, the others are archived.

## Cleared when

The record is accepted or rejected.

## Decided

Option A, 2026-10-15: the record is accepted; the governed stories can be accepted and planned into waves after 0.8.0.
