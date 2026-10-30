---
format: aep.planning-md/3
id: story:translated-refusal-target-status
kind: story
status: implemented
title: A translated call refused after the target answered records the target status
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/tests/translation.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:19Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:19Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-30T04:36:59Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

A translated call whose target answered outside 2xx records that target status in its usage record
and counts one `llmgw_route_upstream_status_failures_total` for its route, as a relayed call does.

## Why

`docs/gateway.md` "Counters and usage records" says the record carries the status the target
answered with, absent only when no target answered, and that the route series counts a target that
answered outside 2xx. In `crates/llm-gateway/src/relay.rs:1317` the `Err(code)` branch returns
before `call.target_status` is set at `:1321`, and `metrics.rs:410-415` counts only `Some`.

Found by the adversarial review of 2026-10-30 (claim R2), proven by the test below failing on main
70841f2a: upstream 429, client 502, status-failure counter 0.

## Acceptance

`translation_a_call_the_target_answered_429_records_the_target_status` in
`crates/llm-gateway-cli/tests/translation.rs` (red on 70841f2a) and
`translate_a_refusal_after_the_target_answered_counts_a_route_status_failure` in
`crates/llm-gateway/tests/translation_port.rs` pass; every other test in those files still
passes. The route-counter assertion moved to the port test because, after
story:metrics-declared-wire-pairs, an undeclared wire has no route series.

## Scope (inferred)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway-cli/tests/translation.rs`, `CHANGELOG.md`.
