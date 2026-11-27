---
format: aep.planning-md/3
id: review-result:spend-ceilings-readable-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on GET /v1/limits (2026-11-27)
relations:
- reviews: story:spend-ceilings-readable
revision: 1
---
## Adversary pass 1 on `GET /v1/limits` (2026-11-27)

Target: branch `feat/limits-read` at 826251872. Red tests: commit a30e97a0a (taken into the branch
as 288c41eba). `b10x-llm-runpod` with the cases: 220 run, 218 passed, 2 failed.

Findings, all introduced by the change:

- F1, warning (`crates/llm-runpod/src/pool.rs`, `spend_ceilings`): the read set the live pods
  without the step a start's check takes first (asking which pods still exist), so after a pod
  ended out of band the read reported 0.5 USD left while a 1 USD start at the same instant was
  admitted. Test `adv_limits_the_read_and_the_next_start_agree_after_a_pod_ended_out_of_band`.
  Fixed in 983cfac80: the read takes the same step.
- F2, warning (`docs/gateway.md`): the daily row counts a live pod's whole run, also before the
  UTC day began, while the docs said the window starts again from nothing. The enforcement has
  always counted so; fixed in 983cfac80 by the docs and spec, and the test
  `adv_limits_daily_spent_counts_a_live_pods_whole_run_so_far` asserts the documented count.
- F3, warning (mutant): no test of the change read with a live pod; dropping `set_live_spend`
  from the read left all eight tests green. Test
  `adv_limits_the_read_counts_a_live_pod_as_the_check_does` kept.
- J1, note: a start over both ceilings was recorded on the daily row only. Fixed in 983cfac80;
  test `limits_a_start_past_both_ceilings_is_the_last_refusal_of_both`.
- J2, note: each read makes one billing request under the pool's lock, uncoalesced. Documented in
  983cfac80 (one listing and one billing request, no caching; calls and wakes wait behind it).

Attacked without a finding: the credential check first; method, body and readiness checks before
any spend read; `?query`, a trailing slash and percent-encoding; no account or pod id in the
answer; no ledger write, start or creation by a read; left never below 0 and micro-USD rounding
at the boundary; the daily window at midnight; the read waiting behind a start on the same lock;
every CLI wrapper passing the rows through.

After the fixes: `task check` exit 0, 1514 tests run, 1514 passed, 1 skipped; conformance 436 of
436 on three runs.
