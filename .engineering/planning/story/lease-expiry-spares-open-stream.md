---
format: aep.planning-md/3
id: story:lease-expiry-spares-open-stream
kind: story
status: active
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
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:23Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:23Z", actor: "human:timo", revision: 4}
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

`review_h2_an_open_stream_survives_a_step_that_comes_after_its_lease_expired` in
`crates/llm-runpod/tests/runpod.rs` passes; every other test in `llm-runpod` and `llm-provision`
still passes; `docs/hosting.md` states what a lease expiry does to a pod with a request in flight.

## Scope (inferred)

`crates/llm-runpod/src/pool.rs`, `crates/llm-provision/src/controller.rs`,
`crates/llm-runpod/tests/runpod.rs`, `docs/hosting.md`, `CHANGELOG.md`.
