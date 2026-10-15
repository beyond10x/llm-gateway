---
format: aep.planning-md/3
id: story:credential-buffers-zeroed
kind: story
status: draft
title: No buffer holding a credential is freed unzeroed
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

No buffer of the gateway or the binary that holds a credential's bytes is freed unzeroed: the
client's request head, and every key-reading buffer of the binary, is sized before a credential
enters it or zeroed on every path that lets it go.

## Why

The output clamp's fix sized the target request head before the bearer is written and zeroes it
on drop (`review-result:adversary-w08-max-tokens-clamp-pass-1`, finding 5). The client's request
head buffer (`crates/llm-gateway/src/server.rs`, the head read loop) grows with
`extend_from_slice` while it holds the owner's credential and is never zeroed; whether the key
readers of `crates/llm-gateway-cli/src/keys.rs` and `hosted.rs` reallocate is unchecked.

## Acceptance

1. The client's request head buffer is zeroed when the request is done, on every return path,
   refusals included, and a reallocation never frees a copy holding credential bytes.
2. The key readers of the binary are read for the same pattern; each one that can reallocate or
   drop unzeroed is fixed, and each one that cannot is named in the commit message.
3. A test or a debug assertion per buffer holds it.

## Scope (inferred)

`crates/llm-gateway/src/server.rs`, `crates/llm-gateway-cli/src/keys.rs`,
`crates/llm-gateway-cli/src/hosted.rs`, `CHANGELOG.md`.
