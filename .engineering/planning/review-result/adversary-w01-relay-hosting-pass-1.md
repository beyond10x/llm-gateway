---
format: aep.planning-md/3
id: review-result:adversary-w01-relay-hosting-pass-1
kind: review-result
status: active
title: Wave 2026-10-30-w01 adversary pass, relay and hosting units, pass 1
relations:
- reviews: story:byte-relay-client-gone
- reviews: story:target-header-obs-text
- reviews: story:translated-refusal-target-status
- reviews: story:metrics-declared-wire-pairs
- reviews: story:lease-expiry-spares-open-stream
revision: 1
---
Adversary pass on wave 2026-10-30-w01 at c053d096: the relay (byte-relay-client-gone,
target-header-obs-text, translated-refusal-target-status, metrics-declared-wire-pairs) and hosting
(lease-expiry-spares-open-stream) changes. Verdict NEEDS-CHANGE: two red tests, two judgement
findings. Targets 3 (translated target status), 4 (declared route series) and 5 (deferred lease
stop) held.

Red tests, in `crates/llm-gateway/tests/wire_relay.rs`:

- `adversary_a_pipelining_client_that_left_releases_the_target_before_the_answer`: the client
  watch reads unread pipelined bytes as a live client, so the FIN behind them is never seen.
- `adversary_a_non_utf8_byte_in_a_target_chunk_extension_is_relayed`: the chunk-size line is read
  as UTF-8, so obs-text in a quoted chunk extension cuts a 200 answer (pre-existing).

Disposition: all four fixed in the wave on impl/w01-relay-fixes.

```findings
- file: crates/llm-gateway/src/relay.rs
  line: 1476
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the client watch treats unread pipelined bytes as a live client, so a pipelining client that closed keeps the target held until it answers
- file: crates/llm-gateway/src/relay.rs
  line: 1194
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: a target chunk-size line whose quoted chunk-extension carries obs-text fails the UTF-8 read and cuts a 200 answer
- file: checks/conformance/src/gateway.rs
  line: 2699
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the conformance client half-closes by default, which the relay now treats as a gone client, so relay scenarios pass only by answering inside the 100 ms watch
- file: crates/llm-gateway/src/relay.rs
  line: 1503
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the watch shuts down only a plain TcpStream target, so the Runpod pod the story's Why names, reached over TLS, is still held after its client left
```
