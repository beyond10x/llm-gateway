---
format: aep.planning-md/3
id: review-result:adversary-w08-effort-default-pass-1
kind: review-result
status: active
title: Wave 2026-10-15-w01 adversary, story:effort-default, pass 1
relations:
- reviews: story:effort-default
revision: 1
---
## Pass

Wave 2026-10-15-w01 adversary, story:effort-default, pass 1, against `3a0cb5b` on
`impl/effort-default`. The adversary added four cases to
`crates/llm-gateway/tests/wire_relay.rs` (committed `3558eca`), all green:
`test result: ok. 4 passed; 0 failed`.

| Case | Now |
|---|---|
| `effort_adv_insertion_keeps_every_body_valid_and_follows_step_6` | green |
| `effort_adv_a_32_mib_body_naming_none_grows_past_the_bound_with_a_matching_length` | green |
| `effort_adv_a_chunked_body_naming_none_gains_the_effort_and_a_matching_length` | green |
| `effort_adv_chat_and_responses_bodies_at_the_bound_gain_no_effort` | green |

Attacked without a break: insertion into an empty `output_config`, one with trailing
whitespace and line breaks, one whose last member is a nested object, and one holding braces
inside strings or non-ASCII keys; `output_config` as an array, `null`, a number or a boolean;
`chat_template_kwargs` as a string, `null` or an array; an effort key only inside a nested
object; a duplicate top-level `output_config` whose first names the effort; a 32 MiB body
growing past the bound with a matching `content-length`; a chunked body reaching the target
decoded with a matching length; the chat and responses wires byte-identical at the bound.
No findings.
