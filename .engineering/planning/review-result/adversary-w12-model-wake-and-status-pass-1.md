---
format: aep.planning-md/3
id: review-result:adversary-w12-model-wake-and-status-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:model-wake-and-status, pass 1
relations:
- reviews: story:model-wake-and-status
revision: 1
---
unit: story:model-wake-and-status, adversary pass 1, covering adv/model-wake-and-status at a68d85b (implementation 4dae621 plus tests only)
verdict: red (two findings introduced, one note pre-existing)
cases: executed 511→513, red 2
origin: n/a (adversary: introduced 2, pre-existing 1, undecided 0)

1. Red: `crates/llm-gateway-cli/tests/adversary_w12_wake.rs`: in the shipped composition a Runpod model of a provider without a connectors connection is `path-unknown` to a wake and a status (`PoolTargets::wake`/`status`, relaying.rs:621, :652), where docs/hosting.md, docs/gateway.md and the specification promise `target-unavailable` and `stopped`.
2. Red, infeasible in the binary: `crates/llm-gateway/tests/adversary_w12_wake.rs`: an empty `sub_phase` or `profile` from a target source is answered `""` (server.rs:792, :796), though docs/gateway.md says a value is absent, never empty.
3. Note, pre-existing: the shipped-composition check reads a hand-kept list of source files rather than walking `src/`; the move of the stop-time unit test into a `#[cfg(test)]` module opens no reach to the emulator.
4. Attacked and not broken: owner credential in every form; method and body checks; alias encodings; hosted endpoints and routes; wake idempotence; spend check and profile fallback through the one pool step; status makes no provider call; pod-start counting deduplicated; a woken pod's idle window.
5. Decisions (coordinator): findings 1 and 2 are fixed as the contract states; finding 3 is left as is.

```findings
[{"file":"crates/llm-gateway-cli/src/relaying.rs","line":621,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"in the shipped composition a Runpod model of a provider without a connectors connection is path-unknown to a wake and a status, where docs/hosting.md, docs/gateway.md and the spec promise target-unavailable and stopped"},
 {"file":"crates/llm-gateway/src/server.rs","line":792,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"an empty sub_phase or profile from a target source is answered as an empty string although docs/gateway.md says a value is absent, never empty; the binary never produces one"},
 {"file":"crates/llm-gateway-cli/tests/relaying.rs","line":308,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"b8 reads a hand-kept list of source files rather than walking src/, so it cannot tell whether a test-only #[path] module stays under cfg(test); the stop-tests move itself opens no reach to the emulator"}]
```
