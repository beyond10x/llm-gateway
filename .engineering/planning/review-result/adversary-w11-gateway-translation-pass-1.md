---
format: aep.planning-md/3
id: review-result:adversary-w11-gateway-translation-pass-1
kind: review-result
status: active
title: Adversary pass 1, story:gateway-translation (wave 2026-10-16-w02)
relations:
- reviews: story:gateway-translation
revision: 1
---
unit: story:gateway-translation, adv/gateway-translation at 52ba794b (the unit head 498805b3 plus one test commit)
verdict: NEEDS-CHANGE
cases: executed 437→441, red 4
origin: introduced 6 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 (~/.cache/llm-gateway-m2b/adv-translation/suite.log)
needs-coordinator: decide whether findings 2 and 3 are fixed here (a guard in the gateway, a published bound) or in llm (decoder strictness, `MAX_EVENTS`)

**1. Diff stat** (498805b3..HEAD): `crates/llm-gateway-cli/tests/adversary_w02_translation.rs | 345 +` — one test file, nothing else.

**2. Cases** — all in that file, each red when run alone, before the suite ran (`cargo test -p b10x-llm-gateway-cli --test adversary_w02_translation`: 0 passed, 4 failed)

| Case | Red output |
|---|---|
| `adv_translation_a_target_that_drops_after_its_head_has_failed_not_refused` | `streamed, closed before any event: Err(Refused(UpstreamRefused))`, `non-streamed, closed mid-answer: Err(Refused(UpstreamRefused))` |
| `adv_translation_chat_fields_below_the_top_level_are_refused_not_dropped` | `tools[].function.strict: None`, `messages[].reasoning_content: None`, `messages[].content[].cache_control: None`, `messages[].name: None` |
| `adv_translation_an_answer_past_65536_upstream_events_is_written_whole` | answer ends `data: {"error":{"code":"upstream-failed","message":"the model target failed after the answer started"}}` and has no `[DONE]` |
| `adv_translation_a_request_naming_no_limit_leaves_room_for_its_input_in_the_window` | `the upstream is asked for max_tokens 32768 of a 32768-token window` |

**3. Suite** (`RUSTUP_TOOLCHAIN=1.98.0 cargo test -p b10x-llm-gateway -p b10x-llm-gateway-cli --locked --no-fail-fast`): EXIT=101, 437 passed, 4 failed. The 4 failures are exactly the cases above; every other target is green. The 437 comes from this same run with my target's line left out. Clippy `-D warnings` is clean on the file; `cargo fmt` changed only my file.

**4. Findings** (they cover 52ba794b)

| # | file:line | Verdict | What reaches it |
|---|---|---|---|
| 1 | crates/llm-gateway-cli/src/translate.rs:757 | NEEDS-CHANGE | A pod that dies mid-generation, or before its first event, after sending a 200 head (Runpod preemption, OOM). The client reads "refused" and the target is not invalidated. An idle timeout of 120 s after the head is classified the same way (Deadline, Accepted), although `docs/gateway.md` says "timed out" is `upstream-failed`. That case is untested because it needs 120 s. Spec step 5 matches the code; the `RefusalCode` docs and docs/gateway.md do not. |
| 2 | crates/llm-gateway-cli/src/translate.rs:345 | NEEDS-CHANGE | Any chat client sending `strict`, `name` or `cache_control`. Also any chat client that replays the `reasoning_content` the gateway itself writes into chat answers. The drop is in llm's decoder, which promises to refuse every field outside its subset. |
| 3 | docs/gateway.md:466 | NEEDS-CHANGE | An answer of more than 65,536 upstream events (about 65k output tokens; vLLM sends one event per token). The client gets `upstream-failed`, which misnames llm's size bound. The bound is not in the Bounds section. |
| 4 | crates/llm-gateway-cli/src/translate.rs:225 | NEEDS-CHANGE | Every Runpod model whose first wire is `messages` (the binary always declares its `max_model_len`), from a chat or responses client that names no limit. That `max_tokens` = window is sent is measured. That vLLM then answers 500, which invalidates the pod, is only inferred, from docs/gateway.md:272. |
| 5 | crates/llm-gateway-cli/src/translate.rs:804 | CONFIRMED | Code reading only. On a non-streamed request `emit` writes nothing, so a client that disconnects is never noticed. The turn and the target run on for up to 3600 s. |
| 6 | docs/gateway.md:521 | CONFIRMED | Code reading only. Known limits says every encoder refusal "ends the stream with the terminal error", but a refusal before any output is answered with a 502 JSON body (decision 7). |

**5. Attacked and could not break:**
- 24 translated turns at once on one translator ran side by side (probe green, then removed).
- Refusals happen in `prepare`, before `acquire`.
- No redirects, proxy or retries (llm-http `Policy::none`, `retry::never`, `no_proxy`).
- Owner credential: the port carries only the body.
- Diagnostics are fixed strings.
- The byte-relay path is unchanged apart from the extracted `acquire`.
- Chunked framing ends with `0\r\n\r\n`.
- `final_usage` means the stream terminated, not that usage was reported.

**6. Outside the worktree:** ~/.cache/llm-gateway-m2b/adv-translation/suite.log. The session lease `adv-translation-p1` is acquired and released.

**7. Findings block**

```findings
[
{"file":"crates/llm-gateway-cli/src/translate.rs","line":757,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a target that drops or idles past 120 s after its 200 head, before client output, is answered upstream-refused and never invalidated, against the RefusalCode docs and docs/gateway.md"},
{"file":"crates/llm-gateway-cli/src/translate.rs","line":345,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"chat fields below the top level (strict, reasoning_content, cache_control, name) are silently dropped instead of translation-unsupported, breaking design 5"},
{"file":"docs/gateway.md","line":466,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"llm's unpublished 65,536-event SSE bound cuts a long translated answer off with a terminal error misnamed upstream-failed"},
{"file":"crates/llm-gateway-cli/src/translate.rs","line":225,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a request naming no limit reaches a windowed messages upstream with max_tokens equal to the whole window, the overrun the byte relay's clamp prevents"},
{"file":"crates/llm-gateway-cli/src/translate.rs","line":804,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"a non-streamed translated request never notices a disconnected client, so the turn holds its target for up to 3600 s"},
{"file":"docs/gateway.md","line":521,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Known limits says every encoder refusal ends the stream with a terminal error, but a refusal before output is a 502 JSON body"}
]
```
