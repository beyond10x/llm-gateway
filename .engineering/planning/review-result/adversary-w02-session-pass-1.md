---
format: aep.planning-md/3
id: review-result:adversary-w02-session-pass-1
kind: review-result
status: active
title: Wave 2026-10-30-w02 adversary pass, relayed session header, pass 1
relations:
- reviews: story:relayed-session-header
revision: 1
---
Adversary pass on wave 2026-10-30-w02 at 95c5416: story:relayed-session-header. Verdict
NEEDS-CHANGE: one red test, four findings, all introduced by the wave. Held: header name case,
a repeated client session, static header case, interior control and space bytes, the 256/257-byte
bound, non-OpenCode Go endpoints (llm's `is_opencode_go`), IPv4-mapped peers, the output clamp
count, logs and metrics, and the translated path's identifier rule.

Cases, in `crates/llm-gateway/tests/relayed_session.rs`:

- `adv_session_a_value_with_a_whitespace_control_or_non_ascii_byte_at_its_edge_is_dropped`: red
  on 95c5416; all six values (VT, FF, NEL, NBSP, U+3000 at an edge, 256 bytes and FF) reached the
  target trimmed.
- `adv_session_a_translated_request_hands_the_translator_the_callers_session_and_peer`: green;
  fails on a copy where `relay.rs:1401` hands `Caller::default()`, which the 273-case suite missed.

Disposition: findings 1, 2 and 4 fixed in 4515e25 (impl/w02-adv-fix); finding 3 kept, no seam
reaches llm's client for opencode.ai without a live call.

```findings
- file: crates/llm-gateway/src/server.rs
  line: 760
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the session is checked after str::trim strips Unicode whitespace and control bytes (VT, FF, NEL, NBSP, U+3000) at its edges, so a malformed or 257-byte client value reaches the OpenCode Go endpoint as its printable remainder instead of being ignored
- file: crates/llm-gateway/src/relay.rs
  line: 1401
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: no existing test reads the Caller the relay hands a translator; replacing it with Caller::default() leaves the 273-case gateway suite green, and adv_session_a_translated_request_hands_the_translator_the_callers_session_and_peer now catches it
- file: crates/llm-gateway-cli/src/translate.rs
  line: 568
  category: mutant
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: Prepared.session and Prepared.peer reach caller_session through no test, and a test would need llm's client to reach opencode.ai, which has no seam here
- file: docs/gateway.md
  line: 638
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the contract says a malformed session is not refused, but by reading the head parser a NUL or a non-UTF-8 byte in it refuses the whole request request-malformed
```
