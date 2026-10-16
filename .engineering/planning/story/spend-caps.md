---
format: aep.planning-md/3
id: story:spend-caps
kind: story
status: draft
title: A pod start that would pass an account's spend ceiling is refused before any provider call
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
revision: 1
---
## Outcome

A pod start that would pass a provider account's daily or total spend ceiling is refused before
any provider call, and the refusal says which ceiling and by how much.

## Acceptance

1. A provider account in the deployment document may declare `daily_cap_usd` and `total_cap_usd`;
   absent means no ceiling of that kind, a non-positive or non-finite value is `config:value`.
2. Before a create, the gateway computes spend = the account's reported spend (read through
   connectors) + the running estimate of its live pods (price per hour times hours since start) +
   the estimate of the pod about to start for its startup deadline. Over either ceiling: the
   request is refused with a new refusal code, no create is sent, and a counter counts it.
3. A spend read that fails or answers nothing refuses the start (no ceiling is assumed met).
4. `ComputeAuthorization` is no longer fixed `unmetered` when a ceiling is declared; the
   specification models the ledger, and conformance scenarios cover under, at and over each ceiling
   against the emulated provider. No test reaches a provider API.

## Blocked

By `decision-blocker:on-demand-gpu-lifecycle`; live checking also by
`third-party-blocker:runpod-account-unfunded`.
