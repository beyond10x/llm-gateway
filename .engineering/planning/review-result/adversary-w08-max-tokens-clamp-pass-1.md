---
format: aep.planning-md/3
id: review-result:adversary-w08-max-tokens-clamp-pass-1
kind: review-result
status: active
title: Wave 2026-10-15-w01 adversary, story:max-tokens-clamp, pass 1
relations:
- reviews: story:max-tokens-clamp
revision: 1
---
## Pass

Wave 2026-10-15-w01 adversary, story:max-tokens-clamp, pass 1, against `92a2aa7` on
`impl/max-tokens-clamp`. The adversary added eight cases to
`crates/llm-gateway/tests/wire_relay.rs` (committed `23445b8`), all green:
`test result: ok. 8 passed; 0 failed`; the crate's suite 197 passed, clippy clean.

| Case | Now |
|---|---|
| `adversary_mtc_the_count_vllm_really_writes_with_context_management_is_read` | green |
| `adversary_mtc_an_escaped_max_tokens_key_is_counted_and_lowered_in_place` | green |
| `adversary_mtc_the_declared_effort_and_the_clamp_rewrite_one_body_byte_exact` | green |
| `adversary_mtc_a_chunked_or_close_framed_count_is_read_whole` | green |
| `adversary_mtc_a_streamed_request_is_counted_lowered_and_still_streamed` | green |
| `adversary_mtc_the_count_of_a_credential_header_endpoint_carries_its_key_there` | green |
| `adversary_mtc_a_body_at_the_32_mib_bound_is_counted_and_lowered_whole` | green |
| `adversary_mtc_the_count_feeds_no_metric_series` | green |

Attacked without a break: the count's target (the one `acquire` handed out, never on a cold
start), its credential and head, malformed, non-2xx, refused and oversized count answers,
chunked and close-framed answers, byte-exact lowering across the model and effort rewrites,
the 32 MiB bound, the chat and responses wires, models without a window, metrics, the refusal's
body against the docs and Claude Code's compaction phrase, vLLM's tolerance of the count body,
and five mutants of the clamp, each caught. Read against vLLM v0.27.1's source.

Findings 3, 4 and 5 are fixed in the same wave; findings 1 and 2 go to
`story:count-renders-effort`.

```findings
- file: crates/llm-gateway/src/relay.rs
  line: 905
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: vLLM v0.27.1's count skips output_config, so its prompt lacks the reasoning_effort and enable_thinking the relayed request renders; clamping to exactly w - n leaves every clamped request over the window by any undercount, on templates that default thinking off (none deployed today).
- file: crates/llm-gateway/src/relay.rs
  line: 908
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: on the documented Qwen3.8 template the count includes the default xhigh instruction a low or medium request does not render, so requests within about 35 tokens of the window are refused context-exceeded and max_tokens is lowered more than needed.
- file: crates/llm-gateway/tests/fixtures/claude-code/4-count-tokens.response.txt
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the fixture claims vLLM answers a compact {"input_tokens":n}, but v0.27.1 also writes context_management (serving.py:1020-1025); the code reads the real shape, the provenance claim is wrong.
- file: crates/llm-gateway/src/relay.rs
  line: 1003
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the count has no time limit of its own, so a stalled count holds every messages request for up to the target's 120 s read timeout before relaying it, doubling the worst-case delay before the first byte.
- file: crates/llm-gateway/src/relay.rs
  line: 992
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: the credential-bearing head grows after the bearer is copied in, so a reallocation frees an unzeroed copy of the bearer before the final fill(0); the count now runs this path twice per request.
```
