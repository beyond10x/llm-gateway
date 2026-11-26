---
format: aep.planning-md/3
id: story:finished-calls-ending-flake
kind: story
status: active
title: The finished-calls ending test gives one verdict under load
relations:
- decomposes: epic:interaction-records
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T19:02:47Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-26T19:02:47Z", actor: "human:operator", revision: 4}
---
## Outcome

`finished_calls::each_ending_is_named_and_the_cost_is_present_exactly_when_the_record_s_is`
(`crates/llm-gateway/tests/finished_calls.rs`) gives the same verdict on every run, under load as
alone; or, when the order it observes is a race in the gateway, that race is a defect with a red
test of its own and is fixed.

## Why

Seen 2026-11-26 on the owner's host, in `task check` on a branch that does not touch
`crates/llm-gateway`: the test failed once (`finished_calls.rs:763`), with the fourth call's
ending read as `Refused(UpstreamFailed)` where it expects `WireError`:

    left:  [Complete, ClientDisconnected, UpstreamCut, Refused(UpstreamFailed), Refused(UpstreamFailed), Refused(ModelUnknown)]
    right: [Complete, ClientDisconnected, UpstreamCut, WireError, Refused(UpstreamFailed), Refused(ModelUnknown)]

It passed in 8 later runs: 6 alone (3 on main 87c42414, 3 on the branch) and 2 full test steps of
1477 tests. The test was added by `f32bad022` the same day.

## Acceptance

- The cause is named: which reply of the test's scripted upstream the fourth call met, and why.
- A red reproduction that fails on demand (for example by holding the timing the load produced),
  committed as `test: … (red)`.
- Either the test stops depending on that timing, or, if the gateway itself can end a call with
  an event-stream error as `Refused(UpstreamFailed)` depending on timing, that is fixed with its
  own red test.
- The test then passes 50 runs in a row under `cargo nextest run --locked -p b10x-llm-gateway`
  with the workspace's other tests running beside it.
