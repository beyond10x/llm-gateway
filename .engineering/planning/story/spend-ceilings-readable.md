---
format: aep.planning-md/3
id: story:spend-ceilings-readable
kind: story
status: implemented
title: The spend ceilings are readable at GET /v1/limits
relations:
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T15:17:10Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-27T15:17:10Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-27T15:17:11Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Problem

The gateway enforces spend ceilings on pod starts (`[providers.<name>] daily_cap_usd`,
`total_cap_usd`), but nothing reads their state: the dashboard snapshot's `limits` are the
subscription limits of credentials, and `/metrics` has only
`llmgw_pod_start_spend_refusals_total`. A consumer that wakes pods cannot show how much is left
before a wake.

## Outcome

An owner route answers, per declared ceiling, its amount, its window, spent and left as the start
check counts them, and the last refusal by that ceiling.

## Acceptance

`GET /v1/limits` rows from a ledger and billing stub: spent and left; left clamped at 0; the daily
window at a fixed clock; the last refusal after a refused start and null before; an empty list
without a ceiling; refused without the owner credential as `/v1/routes` is; at the boundary the
read and a start agree (spent equal to the cap: left 0 and the start refused; left equal to the
estimate: the start admitted); after a pod ends out of band the read and the next start agree.

Decision: asked by a consumer on 2026-11-27; assigned by the host operator.
