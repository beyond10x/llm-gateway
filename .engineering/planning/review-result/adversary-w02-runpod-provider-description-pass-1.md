---
format: aep.planning-md/3
id: review-result:adversary-w02-runpod-provider-description-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w02 adversary, story:runpod-provider-description, pass 1
relations:
- reviews: story:runpod-provider-description
revision: 1
---
## Report

Adversary pass 1 against `story:runpod-provider-description`, commit 8e05ed4 on
`impl/runpod-provider-description` (base 0856b05). New case
`crates/llm-runpod/tests/adversary_w02.rs`
`adv_w02_a_ready_lease_for_a_running_pod_with_a_refused_id_carries_an_address`, red: `ensure handed
out 9 ready leases with no endpoint for running pod(s)`.

```findings
- file: crates/llm-runpod/src/provider.rs
  line: 152
  category: judgement
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: a running, ready pod whose id the description refuses yields ready StreamLeases with endpoint None and is never retired (adversary_w02.rs:149 red); nothing found that issues such an id
- file: docs/llmgw-capability-matrix.md
  line: 131
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: row B7 still states the pod URL without /v1/ and cites the removed format at provider.rs:146-148
- file: docs/hosting.md
  line: 292
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the crate now transitively links b10x-llm-credentials and tokio via b10x-llm-providers, which the "reads no credential" statements here and at src/lib.rs:27 do not mention
```
