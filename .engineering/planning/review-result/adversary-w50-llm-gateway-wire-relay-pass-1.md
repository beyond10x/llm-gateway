---
format: aep.planning-md/3
id: review-result:adversary-w50-llm-gateway-wire-relay-pass-1
kind: review-result
status: active
title: Wave 2026-10-05-w50 adversary, llm-gateway story:wire-relay, pass 1
relations:
- reviews: story:wire-relay
revision: 1
---
```
unit: story:wire-relay, branch impl/wire-relay at f673028 (the unit commit on top of fa594b7)
verdict: NEEDS-CHANGE
cases: executed 85→95, red 5
origin: introduced 9 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 path (build dir, already deleted)
needs-coordinator: the CI toolchain (1.98.0) is not installed here, so CI's clippy on wire_relay.rs was not checked
```

Cases added: `crates/llm-gateway/tests/adversary_relay.rs` (10 cases, 5 red: trailer section over
the head bound; trailer bytes kept in memory, 90 MiB for one request; `+10` / ` 10` chunk sizes
accepted; a chunked answer to an HTTP/1.0 client; a bare LF in a header value framing a body).
Held: the JSON scanner and in-place rewrite over 24 edge cases; TE/CL conflicts, duplicate and
unknown codings; a declared length over the bound refused before reading; the owner credential and
client headers never reaching the target; target hop-by-hop headers never reaching the client; a
cut stream never looking complete; W7 naming the failed target's own authority; refusal bodies not
leaking target text.

```findings
[
{"file":"crates/llm-gateway/src/relay.rs","line":323,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The client trailer section is read with no total byte bound, so a 20042-byte trailer section past request-head-bytes is accepted and relayed (adversary_relay.rs:371)."},
{"file":"crates/llm-gateway/src/relay.rs","line":235,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"Buffered::fill keeps already-consumed bytes whenever a read ends mid-line, so the gateway held 90 MiB in memory for one request's trailers (adversary_relay.rs:406)."},
{"file":"crates/llm-gateway/src/relay.rs","line":292,"category":"boundary","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"chunk_size accepts '+10' and ' 10', which RFC 9112 chunk-size (1*HEXDIG) forbids (adversary_relay.rs:478)."},
{"file":"crates/llm-gateway/src/relay.rs","line":564,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A relayed answer to an HTTP/1.0 request is sent with transfer-encoding: chunked, which RFC 9112 6.1 forbids (adversary_relay.rs:508)."},
{"file":"crates/llm-gateway/src/server.rs","line":400,"category":"boundary","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A header value holding a bare LF is accepted and now frames a relayed body, a request-smuggling shape that RFC 9110 5.5 requires rejecting (adversary_relay.rs:538)."},
{"file":"crates/llm-gateway/tests/wire_relay.rs","line":703,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"clippy -D warnings on local stable 1.99 rejects six assert!(x.is_empty()) in wire_relay.rs (assert_is_empty); the CI toolchain 1.98.0 was not checked."},
{"file":"crates/llm-gateway/src/relay.rs","line":305,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A truncated, timed-out or badly chunked body is refused as request-malformed with the message 'the request head is not well-formed', naming the wrong part of the request."},
{"file":"crates/llm-gateway/src/relay.rs","line":363,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"A target's endless 1xx heads or chunked trailers (relay.rs:444) are read without bound and keep memory as in the trailer finding; only a misbehaving target reaches it."},
{"file":"crates/llm-gateway/src/relay.rs","line":478,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The whole body must arrive within read_timeout (10 s default), so sending the W1 32 MiB bound needs at least 3.4 MB/s, where llmgw has no body deadline; the limit is documented."}
]
```
