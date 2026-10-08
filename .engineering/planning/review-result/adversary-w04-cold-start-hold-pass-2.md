---
format: aep.planning-md/3
id: review-result:adversary-w04-cold-start-hold-pass-2
kind: review-result
status: active
title: Wave 2026-10-08-w04 adversary, story:cold-start-hold, pass 2
relations:
- reviews: story:cold-start-hold
revision: 1
---
## Pass

Wave 2026-10-08-w04 adversary, story:cold-start-hold, pass 2, against the correction
`60f8c99..2370c3f` on `impl/cold-start-hold`. The adversary committed `277e4b1`
(`crates/llm-gateway-cli/tests/adversary_w04_cold_start_pass2.rs`,
`crates/llm-runpod/tests/adversary_w04_hold.rs`, four cases), one red:
`test result: FAILED. 0 passed; 1 failed`.

| Case | Now |
|---|---|
| `adv2_w6_a_request_that_waits_out_its_hold_on_an_unconfirmed_stop_is_not_model_cold_start` | red: `a request that never had a starting pod to wait on was answered: HTTP/1.1 503 ... retry-after: 30 ... "code":"model-cold-start"` |
| `adv2_l6_a_hold_whose_pod_owes_an_unconfirmed_stop_is_lost` | green; fails on a mutant (`pool.rs:508` without the `StopRequired` clause) the unit's suite let through |
| `adv2_l6_a_hold_that_arrives_during_an_unconfirmed_stop_waits_unbound_then_binds_to_the_replacement` | green |
| `adv2_l6_a_hold_on_a_lost_create_that_allocated_nothing_is_lost` | green |

The correction removed the spec clause giving `model-cold-start` to a request whose pod is still
being stopped for a replacement; the relay still answers `model-cold-start` to an unbound request
whose budget passes against an unconfirmed stop, and `docs/hosting.md:351` still promises it.

Attacked without a break: id reuse (ids are `{alias}-{n}`, never removed); a hold whose pod is
replaced while a new request starts the replacement; the cleanup timer racing a held request;
per-request leaks; mixing `ensure` and `ensure_held`; the stop flag; pass 1's red case (red again
when `!hold.lost()` is removed or a `Hold` is made per iteration).

```findings
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 345
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the corrected W6 spec gives model-cold-start only to a request bound to a starting pod, but an unbound request whose hold passes on an unconfirmed stop is still answered model-cold-start with retry-after 30, which docs/hosting.md also still promises
- file: crates/llm-runpod/src/pool.rs
  line: 507
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the hold rule says a hold is lost once its deployment owes a stop, but the check covers StopRequired only and a hold keeps waiting on an Uncertain record, which also owes one
- file: crates/llm-runpod/src/pool.rs
  line: 510
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a held request bound to a lost create that allocated nothing is refused target-unavailable within one ask, where before the correction it was served by the replacement within its hold
```
