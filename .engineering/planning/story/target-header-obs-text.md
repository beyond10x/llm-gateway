---
format: aep.planning-md/3
id: story:target-header-obs-text
kind: story
status: active
title: A non-UTF-8 byte in an ignored target header is relayed, not a failure
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:19Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:19Z", actor: "human:timo", revision: 4}
---
## Outcome

A target answer whose header carries a non-UTF-8 byte (obs-text, RFC 9110 § 5.5) in a field the
gateway does not read is relayed, and the target is not invalidated. A non-UTF-8 byte in a field the
gateway reads (status line, `Content-Length`, `Transfer-Encoding`, `Content-Type`) is still refused.

## Why

`line()` (`crates/llm-gateway/src/relay.rs:937-943`) refuses any non-UTF-8 line and every target
header passes through it (`relay.rs:1087-1095, 1112`); the failure path invalidates the target
(`relay.rs:1533-1540`). One unusual byte in an ignored header from a proxy in front of a pod turns
every answer into 502 and marks a healthy pod gone. `docs/gateway.md:137` states the UTF-8 rule for
client request heads only; nothing states it for target answers.

Found by the adversarial review of 2026-10-30 (claim R4), proven by the test below failing on main
70841f2a: `X-Note: caf\xe9` turned a 200 into a 502.

## Acceptance

`review_r4_a_non_utf8_byte_in_an_ignored_target_header_is_relayed` in
`crates/llm-gateway/tests/wire_relay.rs` passes; every other test in that file still passes.
`docs/gateway.md` states the rule for target answer heads.

## Scope (inferred)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/tests/wire_relay.rs`, `docs/gateway.md`,
`CHANGELOG.md`.
