---
format: aep.planning-md/3
id: story:test-upstreams-bound-to-their-rig
kind: story
status: draft
title: Every scripted fake upstream of the gateway crate replies only to its own rig
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 2
---
## Outcome

Every scripted fake upstream in `crates/llm-gateway/tests` hands its replies only to the calls of
its own rig, and never connects to a port it no longer holds when it is dropped.

## Why

`story:finished-calls-ending-flake` found that the fake upstream of `tests/finished_calls.rs`
served its scripted replies in connection order to anything that connected to its loopback port,
so a connection from another test process took a reply and the test failed under load. It fixed
that one upstream: a per-upstream token header, any other connection closed without using a reply,
and a drop that wakes its accept loop while it still holds the listener. The same pattern is in
eight other fake upstreams of the crate: `streamed_targets.rs`, `adversary_claude_code_wire.rs`,
`adversary_w05_finished_calls.rs`, `adversary_w10_ingress.rs`, `adversary_local_open.rs`,
`spec_declarations.rs`, `adversary_relay.rs` and `wire_relay.rs`.

## Acceptance

- Each of the eight upstreams replies only to requests carrying its token, or shares one fixed
  upstream with that rule.
- A red test shows a foreign connection taking a reply from one of them before the change.
- `cargo nextest run --locked -p b10x-llm-gateway` passes 50 runs in a row with the workspace's
  other tests running beside it.
