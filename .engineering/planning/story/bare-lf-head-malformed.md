---
format: aep.planning-md/3
id: story:bare-lf-head-malformed
kind: story
status: active
title: A bare-LF request head is request-malformed at any length
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/tests/gateway.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:17Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:17Z", actor: "human:timo", revision: 4}
---
## Outcome

A request head holding a bare LF is refused 400 `request-malformed` whatever its length, including
when no CRLFCRLF arrives before the 8192-byte head bound.

## Why

`docs/gateway.md:263-264` and `:137-139` say a head holding a CR, LF or NUL anywhere but its line
ends is `request-malformed`. The terminator search looks only for CRLFCRLF
(`crates/llm-gateway/src/server.rs:623-628`) and the LF check runs in `Request::parse`
(`server.rs:695-700`) only after one arrives, so a bare-LF head past 8192 bytes is answered 431
`request-too-large` (`server.rs:630-633`), and one under the bound waits for `read_timeout`.

Found by the adversarial review of 2026-10-30 (claim S2), proven by the test below failing on main
70841f2a.

## Acceptance

`review_s2_a_bare_lf_head_past_the_head_bound_is_request_malformed` in
`crates/llm-gateway/tests/gateway.rs` passes; every other test in that file still passes.

## Scope (inferred)

`crates/llm-gateway/src/server.rs`, `crates/llm-gateway/tests/gateway.rs`, `CHANGELOG.md`.
