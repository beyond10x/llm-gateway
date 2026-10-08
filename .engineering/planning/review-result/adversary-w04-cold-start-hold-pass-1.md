---
format: aep.planning-md/3
id: review-result:adversary-w04-cold-start-hold-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w04 adversary, story:cold-start-hold, pass 1
relations:
- reviews: story:cold-start-hold
revision: 1
---
## Pass

Wave 2026-10-08-w04 adversary, story:cold-start-hold, pass 1, against `impl/cold-start-hold` at
`0a9e945` (base `c0961cd`). The adversary committed `60f8c99`
(`crates/llm-gateway-cli/tests/adversary_w04_cold_start.rs`, three cases), one red:
`test result: FAILED. 2 passed; 1 failed` (3 of 3 runs).

| Case | Now |
|---|---|
| `adv_w6_every_request_held_on_a_pod_that_misses_its_startup_deadline_is_target_unavailable` | red: `a request held while its pod missed its startup deadline was answered (creates: 2): HTTP/1.1 503 ... retry-after: 30 ... "code":"model-cold-start"` |
| `adv_w6_a_request_held_on_a_pod_that_exits_is_target_unavailable` | green |
| `adv_w6_every_wire_answers_model_cold_start_with_retry_after_30` | green |

Measured at `relaying.rs:340`: the pool reports `startup-deadline` only to the caller whose step
retired the pod; every other held request sees `Stopping`, which the arm reads as still starting,
so that request starts a second pod and is answered `model-cold-start` for a model whose pod just
failed, against `spec/domains/deployment.yaml` (row W6: any other pool refusal is
`target-unavailable`). Reached by two concurrent requests to a cold model, or by one held request
when the cleanup timer's step retires the pod.

Attacked without a break: hold boundaries (0, equal to `start_wait_seconds`, above it refused);
a single held request whose pod fails; the stop during a hold (`adv2_d2_*`); `retry-after` on all
three wires; the refusal table in `docs/gateway.md`; panics in the cleanup timer.

```findings
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 340
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: when two requests are held on a pod that fails, only the one whose pool step retired it gets target-unavailable; the other sees Stopping, starts a second pod and is answered model-cold-start with retry-after 30
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 311
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the acquire doc comment claims a held request never starts a second pod, but a held request whose pod failed starts its replacement within the same hold
- file: crates/llm-gateway/src/server.rs
  line: 25
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: held requests occupy one of 64 concurrency slots for up to request_hold_seconds, so a burst on one cold model answers overloaded for every other model
```
