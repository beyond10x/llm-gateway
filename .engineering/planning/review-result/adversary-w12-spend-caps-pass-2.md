---
format: aep.planning-md/3
id: review-result:adversary-w12-spend-caps-pass-2
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:spend-caps, pass 2
relations:
- reviews: story:spend-caps
revision: 1
---
unit: story:spend-caps, adversary pass 2, covering adv/spend-caps at 7bee3ec plus 4a4c220 (tests only)
verdict: red (one finding, introduced)
cases: executed 381→383, red 1
origin: n/a (adversary: introduced 1, pre-existing 0, undecided 0)

1. The pass-1 blocker is fixed: every pod create in the shipped composition passes the spend check (`provider.rs:338` runs `admit_spend` before each GPU type; `serve.rs`, `start_connected` and `start_relaying` build their pool through `runpod_pool`).
2. Red: `adversary_w12p2_another_providers_ceiling_does_not_bind_the_connected_account` (crates/llm-gateway-cli/tests/adversary_w12_spend_pass2.rs). `start_connected` keeps every provider, and `spend_policy` takes whichever provider's ceiling, so an unconnected provider's ceiling is checked against the connected account's billing. It refuses more, never less.
3. Green, kept: `adversary_w12p2_a_live_pod_counts_at_its_own_startup_deadline` pins per-model startup deadlines.
4. Attacked and not broken: day start on both transports; rounding (estimate up, equal-to-ceiling admitted); every malformed `billing.pods` answer is unknown and refuses the start; racing starts are serialised by the pool's mutex.
5. Note, no test: a stopped pool pod leaves the running estimate before the provider bills it; `docs/hosting.md` names this lag only for pods the pool holds no record of.
6. Decision (coordinator): the pool binds only the ceiling of the provider whose account it bills; a ceiling on another provider does not bind it. The hosting document names the billing lag of a stopped pod.

```findings
[{"file":"crates/llm-gateway-cli/src/relaying.rs","line":839,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"start_connected keeps every provider in the subset, so spend_policy binds the connected account's pool with a ceiling another, unconnected provider declared for its own account"}]
```
