---
format: aep.planning-md/3
id: review-result:adversary-w11-gateway-translation-pass-2
kind: review-result
status: active
title: Adversary pass 2, story:gateway-translation (wave 2026-10-16-w02)
relations:
- reviews: story:gateway-translation
revision: 1
---
unit: story:gateway-translation, adv/gateway-translation at e72e4b6b (a1639606 plus one test commit)
verdict: NEEDS-CHANGE
cases: executed 450→455, red 5
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 3 (~/.cache/llm-gateway-m2b/adv-translation-p2/{cli-red,gw-red,suite}.log)
needs-coordinator: finding 3 comes from decision 1 itself: "protocol is upstream-failed" also covers a model's bad output, not only a broken target

**1. Diff stat** (a1639606..HEAD): only two test files changed, `crates/llm-gateway-cli/tests/adversary_w02_translation_p2.rs` (+316) and `crates/llm-gateway/tests/adversary_w02_translation_p2.rs` (+142). No implementation file was touched.

**2. Cases.** Each was red when run alone, before the suite ran.

| # | Case | Red output |
|---|---|---|
| 1 | `adv_p2_a_tool_call_past_64_kib_of_arguments_inside_every_published_bound_is_written_whole` | A 60000-byte control is written whole. A 70000-byte call ends `data: {"error":{"code":"translation-too-large",…}}` |
| 2 | `adv_p2_a_messages_upstream_of_a_model_without_a_window_is_asked_for_at_most_16384` | `left: Some(200000) right: Some(16384)` |
| 3 | `adv_p2_a_model_that_calls_an_unpublished_tool_does_not_fail_its_target` | Calling the published tool answers 200. Calling an unpublished one gives `left: Err(UpstreamFailed) right: Err(UpstreamFailed)` |
| 4 | `adv_p2_an_event_of_two_lines_each_under_the_line_bound_is_written_whole` | The answer ends with the `translation-too-large` terminal error |
| 5 | `adv_p2_a_client_gone_before_a_translated_answer_is_not_recorded_relayed_with_status_0` | `recorded Relayed with status 0 (target status None)` |

**3. Suite.** `RUSTUP_TOOLCHAIN=1.98.0 cargo test -p b10x-llm-gateway -p b10x-llm-gateway-cli --locked --no-fail-fast` gave EXIT=101, 450 passed and 5 failed. The 5 failures are exactly the cases above. The 450 is the same run without my two targets. Clippy `-D warnings` and fmt are clean on both files.

**4. Findings** (they cover e72e4b6b)

| # | file:line | Verdict | What reaches it |
|---|---|---|---|
| 1 | docs/gateway.md:741 | NEEDS-CHANGE | Any tool call over 64 KiB on a translated route, such as an agent writing a file. llm's `MAX_TOOL_ARGUMENT_BYTES` (`llm-messages/src/decode.rs`) is not in the published bounds. The terminal message names three bounds the answer did not pass. The byte relay relays the same answer. |
| 2 | crates/llm-gateway-cli/src/translate.rs:555 | NEEDS-CHANGE | A `[hosted.*]` messages endpoint without `context_window` (the key is optional). Decision 4 is applied only when the model declares a window, and the unit's own test pins 200000. Measured: 200000 is sent. Not measured: whether an upstream then refuses it. |
| 3 | crates/llm-gateway-cli/src/translate.rs:880 | NEEDS-CHANGE | A non-streamed request where the model calls a tool that was not offered. llm reports that as `protocol`, so a healthy pod is invalidated and the client is told "the next request asks for a replacement". This follows decision 1 exactly, but neither the docs nor the `UpstreamFailed` doc comment name this case. |
| 4 | docs/gateway.md:752 | INFEASIBLE | llm's 1 MiB limit applies to an event's joined data, not to one line. I built the trigger myself (one event's data split over two lines); I found no upstream that does this. It is a doc-wording fix. |
| 5 | crates/llm-gateway/src/relay.rs:1096 | CONFIRMED | Every client that gives up on a non-streamed translated answer. It is counted as a relayed call with status 0, although the docs define Relayed as "the target's answer reached the client". |

**5. Attacked and could not break:**
- **Key guard.** Chat and Responses allow exactly the keys llm reads at each nested level, and Messages closes every level itself. Checked against llm's source, so there are no false refusals.
- **Half-close.** `gone()` is only asked when the request is not streamed. A streamed client that half-closes is not affected (code reading only).
- **Bounds at the number and one past it.** The unit's test measures all three, and they hold.
- **Status codes.** A 413 maps to `Refused`, which gives `upstream-refused`, so it is never misread as too-large.
- **Pass-1 tests.** Only the 65536-event case was changed, and only in the way decision 3 says.
- **Non-streamed cancellation.** A client that leaves cuts the upstream connection and nothing is written to it.

**6. Outside the worktree:** the three logs above. The session lease `adv-translation-p2` was taken and released.

**7. Findings block**

```findings
[
{"file":"docs/gateway.md","line":741,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"llm's unpublished 65536-byte streamed tool-argument bound cuts off a tool call that passes every published bound of the translated path, with a terminal message naming bounds it did not pass"},
{"file":"crates/llm-gateway-cli/src/translate.rs","line":555,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"decision 4 is applied only to models with a declared window, so a model without one asks its messages upstream for max_tokens 200000 instead of 16384"},
{"file":"crates/llm-gateway-cli/src/translate.rs","line":880,"category":"judgement","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a model's invalid output (an unpublished tool call), which llm reports as protocol, before any output invalidates a healthy target as upstream-failed"},
{"file":"docs/gateway.md","line":752,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the 1048576 bound applies to an event's joined data, not one line, so a multi-line event under the published line bound is refused translation-too-large"},
{"file":"crates/llm-gateway/src/relay.rs","line":1096,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"a non-streamed translated request whose client left before any answer is recorded Relayed with status 0, contrary to the UsageRecord contract"}
]
```
