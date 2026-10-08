---
format: aep.planning-md/3
id: review-result:adversary-w03-gateway-deployment-pass-2
kind: review-result
status: active
title: Wave 2026-10-08-w03 adversary, story:gateway-deployment, pass 2
relations:
- reviews: story:gateway-deployment
revision: 1
---
## Pass

Wave 2026-10-08-w03 adversary, story:gateway-deployment, pass 2, against `impl/gateway-deployment`
at `cc152f8` (the answer to pass 1). The adversary committed `13d4c97`
(`crates/llm-gateway-cli/tests/adversary_w03_pass2.rs`, three cases): `FAILED. 2 passed; 1 failed`.
The pass-1 cases were not weakened (`git diff f28f34f...cc152f8 -- crates/llm-gateway-cli/tests/adversary_w03.rs` is empty).

| Case | Status | Output |
|---|---|---|
| `adv2_d2_a_request_accepted_before_the_stop_is_relayed_to_its_ready_pod` | red | `an accepted request to a ready pod was refused by the stop: HTTP/1.1 503 Service Unavailable … {"error":{"code":"target-unavailable",…}}` |
| `adv2_d2_a_request_waiting_for_its_pod_is_answered_target_unavailable_at_the_stop` | green | a waiting request gets a prompt 503 `target-unavailable` |
| `adv2_w7_a_report_about_a_replaced_pod_never_stops_its_replacement` | green | stale and repeated reports return false; 2 pods created |

Attacked without a break: invalidate races (stale report, repeated report, not-ready or replaced
deployment); the idle clock's `fetch_max(now)` at first readiness against the orphan sweep and
`restore`; the signal path sets the stop flag; the seam test checks behaviour.

```findings
[
  {"file": "crates/llm-gateway-cli/src/relaying.rs", "line": 315, "category": "contract-drift", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "acquire checks the stop flag before its first ask of the pool, so a request accepted before the stop is answered target-unavailable instead of being relayed to its ready pod"}
]
```
