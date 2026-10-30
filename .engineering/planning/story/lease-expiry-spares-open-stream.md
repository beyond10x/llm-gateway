---
format: aep.planning-md/3
id: story:lease-expiry-spares-open-stream
kind: story
status: implemented
title: A lease expiry does not stop a pod with a request in flight
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-provision/src/controller.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: cited
  path: crates/llm-runpod/tests/runpod.rs
- confidence: cited
  path: docs/hosting.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:23Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-30T04:37:00Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

A pod with a request in flight is not terminated by a lease expiry: when a pool step comes later than
`lease_ms` while a `StreamLease` is open, the step submits no stop for that pod.

## Why

`docs/hosting.md:312` says the idle reaper never reaps with a request in flight. Lease expiry marks
the record `StopRequired` (`crates/llm-provision/src/controller.rs:353-357`) and
`submit_owed_stops` (`crates/llm-runpod/src/pool.rs:1002-1012`) carries it out without looking at
`in_flight`, so a step stalled past `lease_ms` (300 s, `crates/llm-gateway-cli/src/relaying.rs:63`)
cuts a live stream and the next request pays a cold start.

Found by the adversarial review of 2026-10-30 (claim H2), proven by the test below failing on main
70841f2a: `terminations == ["pod1"]` with a stream open.

## Acceptance

`a_lease_expiry_does_not_stop_the_pod_of_an_open_stream` (red on 70841f2a),
`a_lease_expiry_stops_the_pod_once_its_last_stream_ends` and
`an_open_stream_does_not_spare_a_lease_expired_pod_past_its_lifetime_ceiling` in
`crates/llm-runpod/tests/runpod.rs` pass; every other test in `llm-runpod` and `llm-provision`
still passes; `docs/hosting.md` states what a lease expiry does to a pod with a request in flight.

## Scope (inferred)

`crates/llm-runpod/src/pool.rs`, `crates/llm-provision/src/controller.rs`,
`crates/llm-runpod/tests/runpod.rs`, `docs/hosting.md`, `CHANGELOG.md`.
