---
format: aep.planning-md/3
id: story:byte-relay-client-gone
kind: story
status: active
title: A relayed non-streamed call releases the target when its client leaves
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
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:18Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:18Z", actor: "human:timo", revision: 4}
---
## Outcome

When a client of a relayed (not translated), non-streamed call closes its connection while the
target is still generating, the gateway closes its connection to the target within 1 s instead of
holding it until the answer arrives.

## Why

The translated path cancels a turn whose client left (`TranslatedAnswer::gone`, `docs/gateway.md`
Translation section). The byte relay blocks in `read_answer_head`
(`crates/llm-gateway/src/relay.rs:1533`) and notices a gone client only when a write fails
(`relay.rs:1739-1741`), so a GPU target keeps generating, and billing, for an answer nobody reads.

Found by the adversarial review of 2026-10-30 (claim R1), proven by the test below failing on main
70841f2a: the target was still held 3 s after the client left.

## Acceptance

`review_r1_a_non_streamed_client_that_left_releases_the_target_before_the_answer` in
`crates/llm-gateway/tests/wire_relay.rs` passes; every other test in that file still passes.
`docs/gateway.md` states the behaviour for the byte relay.

## Scope (inferred)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/tests/wire_relay.rs`, `docs/gateway.md`,
`CHANGELOG.md`.
