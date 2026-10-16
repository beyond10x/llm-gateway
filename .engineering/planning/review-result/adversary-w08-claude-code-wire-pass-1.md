---
format: aep.planning-md/3
id: review-result:adversary-w08-claude-code-wire-pass-1
kind: review-result
status: active
title: Wave 2026-10-15-w01 adversary, story:claude-code-wire, pass 1
relations:
- reviews: story:claude-code-wire
revision: 1
---
## Pass

Wave 2026-10-15-w01 adversary, story:claude-code-wire, pass 1, against `cfa37e9` on
`impl/claude-code-wire`. The adversary wrote
`crates/llm-gateway/tests/adversary_claude_code_wire.rs`, eight cases, all green:
`test result: ok. 8 passed; 0 failed`.

| Case | Now |
|---|---|
| `adv_ccw_max_written_with_escapes_in_key_or_value_is_mapped` | green |
| `adv_ccw_max_anywhere_but_the_two_fields_or_not_a_string_is_unchanged` | green |
| `adv_ccw_every_duplicate_occurrence_of_max_is_mapped` | green |
| `adv_ccw_every_other_byte_of_a_pretty_body_is_unchanged` | green |
| `adv_ccw_other_wires_pass_an_escaped_max_unchanged` | green |
| `adv_ccw_a_messages_body_at_the_bound_with_max_reaches_the_pod_mapped` | green |
| `adv_ccw_head_api_hello_with_both_credential_headers_or_x_api_key_alone` | green |
| `adv_ccw_probe_count_tokens_is_refused_and_asks_for_no_target` | green (a measurement: 405 `method-not-allowed`) |

Attacked without a break: the effort mapping under escaped keys and values, nesting, wrong keys,
non-string values, case and whitespace variants, duplicate keys; `max` unchanged on the chat and
responses wires; byte-exactness (whitespace, number spellings, escapes, raw UTF-8,
`cache_control`, key order); a 32 MiB body growing past the bound after the rewrite;
`HEAD /api/hello` under every credential header; the docs' citations of vLLM v0.27.1, the model
template and the protocol page.

```findings
- file: docs/gateway.md
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the Claude Code section omits POST /v1/messages/count_tokens, which the pinned vLLM serves and Claude Code calls; the gateway answers it 405 method-not-allowed, undocumented
```
