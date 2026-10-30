---
format: aep.planning-md/3
id: story:metrics-declared-wire-pairs
kind: story
status: active
title: Route series register only the wire pairs a model declares
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/prometheus_metrics.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:19Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:20Z", actor: "human:timo", revision: 4}
---
## Outcome

With a translator composed, route series are registered at zero only for the (model, wire) pairs the
model declares, and a translated request on another wire feeds the process-wide series only.

## Why

`docs/gateway.md:809-811` and `spec/domains/telemetry.yaml:17-18` say exactly that. `register`
(`crates/llm-gateway/src/relay.rs:756-766`) uses `Wire::ALL` whenever the relay translates, and the
binary always composes a translator (`crates/llm-gateway-cli/src/relaying.rs:1180`), so a chat-only
model shows `wire="responses"` and `wire="messages"` route series.

Found by the adversarial review of 2026-10-30 (claim M2), proven by the test below failing on main
70841f2a.

## Acceptance

`review_m2_a_translating_relay_registers_only_the_declared_wire_pairs` in
`crates/llm-gateway/tests/prometheus_metrics.rs` passes; every other test in that file and the
conformance suite still pass.

## Scope (inferred)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/src/metrics.rs`,
`crates/llm-gateway/tests/prometheus_metrics.rs`, `CHANGELOG.md`.
