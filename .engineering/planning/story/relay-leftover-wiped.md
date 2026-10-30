---
format: aep.planning-md/3
id: story:relay-leftover-wiped
kind: story
status: draft
title: Bytes past a relayed head are overwritten when the relay lets them go
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- derived_from: story:credential-buffers-zeroed
revision: 2
---
## Outcome

The bytes the gateway read past a request head reach the relay and are overwritten when the relay lets them
go. That is the start of a body, or a pipelining client's next request, credential included. Today `serve`
hands the head reader's `Leftover` vector unchanged to the relay's `Buffered` reader
(`crates/llm-gateway/src/relay.rs`, `struct Buffered`). The reader drains and grows the vector while it reads
the body, and frees it without overwriting it.

## Why

story:credential-buffers-zeroed sized and wiped every buffer that holds a credential on the paths the gateway
answers itself, and the bytes past the head are now a `Leftover` that is overwritten on drop. The relay was
outside that story's files. That story's security review (`review-result:security-w06-credential-buffers`) and
its verification record (`docs/verification/2026-10-30-credential-buffers-zeroed.md`, Residuals) leave this
as the one gateway-owned holder that remains.

## Acceptance

- The process-memory scan in `crates/llm-gateway/tests/security_w06_head_buffer.rs`, extended to a pipelined
  pair whose first request is relayed, finds the second request's credential nowhere once the gateway has
  answered both. The case is red before the change.
- The relay's body bytes are unchanged for every body framing the relay tests cover today.
- The verification record moves the relay from Residuals into the per-buffer table.
