---
format: aep.planning-md/3
id: story:relay-read-interrupted
kind: story
status: active
title: An interrupted read is retried, not answered upstream-failed
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_claude_code_wire.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/spec_declarations.rs
- confidence: cited
  path: crates/llm-gateway/tests/wire_relay.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T20:20:52Z", actor: "human:timo", revision: 8}
- {from: "proposed", to: "active", at: "2026-10-16T20:20:52Z", actor: "human:timo", revision: 9}
---
## Outcome

An interrupted read (EINTR) on the relay's sockets is retried within its deadline, not answered
`upstream-failed` (502) or refused as a client fault.

## Why

A socket with a read timeout returns EINTR when its thread takes a signal or its process is
stopped and continued. The relay took that for a failed peer, which made
`effort_adv_chat_and_responses_bodies_at_the_bound_gain_no_effort` fail intermittently with a 502.

## Acceptance

`relay::tests::an_interrupted_read_of_the_answer_is_retried` passes: an answer whose head read is
interrupted is relayed whole. `Buffered::fill`, `read_head` and `linger` retry an interrupted read
and their deadlines still bound them. The test pods in `wire_relay.rs`, `adversary_relay.rs`,
`adversary_claude_code_wire.rs` and `spec_declarations.rs` retry theirs too.

## Delivered by

Commit 847e636 on `fix/relay-read-interrupted`: stopping and continuing the test process every
50 ms failed 4 of 10 runs before it and 0 of 20 after.

## Scope (cited)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/src/server.rs`,
`crates/llm-gateway/tests/{wire_relay,adversary_relay,adversary_claude_code_wire,spec_declarations}.rs`.
