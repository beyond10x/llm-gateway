---
format: aep.planning-md/3
id: review-result:security-w12-spend-caps-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 security review, story:spend-caps, pass 1
relations:
- reviews: story:spend-caps
revision: 1
---
unit: story:spend-caps, security review pass 1, covering sec/spend-caps at f71da82 (impl/spend-caps 28e4787 plus one test commit)
verdict: red (invariant 1 broken in the shipped composition)
cases: executed 374→377, red 2
origin: n/a (introduced 2, pre-existing 0, undecided 0)
wrote-outside-worktree: run scripts and logs in the wave scratch directory

Invariants:
1. No create without an admitting check: broken in the shipped composition (start_connected builds the pool without the spend policy); held on every other path. The only production create is behind admit_spend inside the GPU loop; the replacement of an exited pod and the GPU fallback each read spend first (green case).
2. Fail closed: held except a connectors result with no status, which is taken as success and an empty body as zero spend.
3. No silent disable through config: held (a ceiling needs every price, 0 and NaN refused, one capped provider).
4. The client refusal carries no amount; the amount is only in the warn event; billing.pods is a read with no approval: held by reading.
5. No network beyond loopback: held.

Judgement: the total ceiling sums billing.pods over the provider's default time window, which may be bounded; unverifiable offline.

Decisions (coordinator): finding 1 is the same blocker as the adversary's; a result with no status is unanswered (fail closed); the default time window goes into docs/hosting.md's list of billing assumptions not checked against a live account.

```findings
[{"file":"crates/llm-gateway-cli/src/relaying.rs","line":855,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"start_connected, the shipped binary's only composition, builds the pool without with_spend, so a declared ceiling never gates pod.create"},
 {"file":"crates/llm-runpod/src/transport/connectors.rs","line":826,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"a billing.pods result without a status passes body() and an empty body reads as zero spend instead of unanswered"},
 {"file":"crates/llm-runpod/src/transport/connectors.rs","line":826,"category":"judgement","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"the total ceiling sums billing.pods over the provider's default time window, which may be bounded and so understate total spend"}]
```
