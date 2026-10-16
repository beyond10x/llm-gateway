---
format: aep.planning-md/3
id: story:spend-caps
kind: story
status: active
title: A pod start that would pass an account's spend ceiling is refused before any provider call
relations:
- decomposes: epic:hosting
- informed_by: architecture-decision-record:on-demand-gpu-lifecycle
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: checks/conformance/src/hosting.rs
- confidence: cited
  path: checks/conformance/src/runpod.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway/src/error.rs
- confidence: inferred
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-provision/src/spec.rs
- confidence: cited
  path: crates/llm-runpod/src/emulated.rs
- confidence: inferred
  path: crates/llm-runpod/src/pool.rs
- confidence: cited
  path: crates/llm-runpod/src/transport.rs
- confidence: cited
  path: crates/llm-runpod/src/transport/connectors.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/hosting.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/hosting.yaml
- confidence: inferred
  path: spec/domains/runpod.yaml
revision: 21
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 20}
- {from: "proposed", to: "active", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 21}
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

Not blocked.

The gateway's on-demand GPU lifecycle is decided (`architecture-decision-record:on-demand-gpu-lifecycle`,
accepted; `decision-blocker:on-demand-gpu-lifecycle` cleared). This story is delivered offline:
`EmulatedRunpod` with a scriptable spend answer, the `connectors-fixture` binary and conformance
scenarios. No test reaches a provider API.

The account's reported spend has no read today: `RunpodTransport` lists, creates and terminates
pods only, and the connectors Runpod catalog offers `pod.create`, `pods.list` and `pod.terminate`
(Runpod's own API has `GET /billing/pods`, operation `PodBilling`). `ConnectorsRunpod` therefore
names the read it needs and fails closed until connectors offers it: with a ceiling declared, a
spend read that is unavailable refuses the start (acceptance 3), so a declared ceiling is never
silently unchecked. Checking against a live, funded account stays with
`third-party-blocker:runpod-account-unfunded` and `story:client-qualification`.
