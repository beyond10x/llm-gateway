# Claude Code on the messages wire, 2026-10-15

What `story:claude-code-wire` settled about Claude Code 2.1.293 reaching a Qwen3.8-27B-FP8 model
behind the pinned vLLM (`vllm/vllm-openai:v0.27.1`, `--enable-auto-tool-choice
--tool-call-parser qwen3_coder`) through this gateway, and what it left for a live record. It is
settled offline, from published documents and vLLM's source. No pod was started, no Claude Code
session reached a model, nothing was paid for, and no byte below was captured: the fixtures under
`crates/llm-gateway/tests/fixtures/claude-code/` are authored from the sources listed here, each
headed by the ones it was derived from.

## Sources read

All read on 2026-10-15. vLLM paths are under
<https://github.com/vllm-project/vllm/blob/v0.27.1/> (tag `v0.27.1`, commit
`6e448d0ea9bf3d88d898b65449ca6dc2aec170ac`).

| Source | What it settles |
| --- | --- |
| <https://code.claude.com/docs/en/llm-gateway-protocol> | Inference is `POST /v1/messages?beta=true`; `HEAD /api/hello` is "a `HEAD /api/hello` connection-warming probe", startup traffic a gateway "can reject without breaking anything"; `anthropic-version` (`2023-06-01`) and `anthropic-beta` are sent and, for an Anthropic upstream, should be forwarded; a model id Claude Code does not recognise receives `thinking: {"type": "adaptive"}`, effort and context management; `output_config` carries effort, the structured-output format and a task budget, the format switched off by `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1` (v2.1.288 or later) or `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS=1`; `cache_control` is sent on `system` blocks and `messages` entries; the stream idle watchdog can trip when no `ping` arrives during a long thinking pause; a `thinking` rejection is retried without it, an `output_config.effort` rejection is recognised by a `400` naming `output_config.effort` with `Extra inputs are not permitted` |
| <https://platform.claude.com/docs/en/build-with-claude/effort> | The effort levels `low`, `medium`, `high`, `xhigh`, `max` |
| <https://platform.claude.com/docs/en/build-with-claude/structured-outputs> | `output_config.format = {"type": "json_schema", "schema": {...}}` is the generally available shape; the top-level `output_format` with `structured-outputs-2025-11-13` is the deprecated beta's |
| <https://platform.claude.com/docs/en/build-with-claude/thinking-steering-and-cost> | What `{"type": "adaptive"}` asks of an Anthropic model |
| <https://code.claude.com/docs/en/env-vars> | `CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=1` sends effort "even when Claude Code does not recognize the model ID as effort-capable"; `CLAUDE_CODE_DISABLE_THINKING=1` omits `thinking` |
| vLLM `vllm/entrypoints/anthropic/protocol.py:104-115` | `output_config.effort` admits `low`, `medium`, `high`, `xhigh`, `max`; `output_config.format` is `{"type": "json_schema", "schema": …}` |
| vLLM `vllm/entrypoints/anthropic/protocol.py:118-178` | The messages request model declares no `thinking` and no `output_format`; a plain pydantic model, it drops unknown keys, so both are ignored and never refused |
| vLLM `vllm/entrypoints/anthropic/serving.py:496-514` | `output_config.format` with a schema becomes `response_format` (structured output); `output_config.effort` becomes `reasoning_effort` |
| vLLM `vllm/entrypoints/openai/chat_completion/protocol.py:555-590` | A `reasoning_effort` other than `none` sets `enable_thinking`, unless the request's own `chat_template_kwargs` name it |
| vLLM `vllm/entrypoints/anthropic/api_router.py` (read whole) and `serving.py` | No request header is read: neither `anthropic-version` nor `anthropic-beta`. No `/api/hello` route exists in the tree |
| vLLM `vllm/entrypoints/anthropic/serving.py:95-96`, `617-671`, `673-996`; `protocol.py:181-240` | The answer shapes the response fixtures follow: `event: <type>\ndata: <json>\n\n`, compact JSON in declared field order, a thinking block whose `signature_delta` is written when it closes, a `tool_use` block, `message_delta` with the stop reason, `message_stop`, and no `ping` event |
| vLLM `vllm/entrypoints/serve/utils/server_utils.py:498-541` | A request that fails validation is a `400` with an OpenAI-shaped error body, not Anthropic's `{"type":"error",…}` |
| `Qwen/Qwen3.8-27B-FP8` `chat_template.jinja` at Hugging Face commit `017b9c7af6b5689d5dd426a76e0bc077eb5ca20a`, lines 46-55 | The template accepts the efforts `xhigh` (its default), `medium` and `low`, and raises on any other |

## What was settled, and where it is held

| Point | Settled | Held by |
| --- | --- | --- |
| D2: `HEAD /api/hello` | Kept: `credential-absent` (401) without the owner credential, `path-unknown` (404) with it, no body either way. The protocol page says the probe can be rejected without breaking anything | `claude_code_the_startup_probe_head_api_hello_is_refused_and_asks_for_no_target` in `crates/llm-gateway/tests/wire_relay.rs`; scenario `claude-code-the-startup-probe-head-api-hello-is-refused`; `docs/gateway.md` "The relay" |
| D3: `thinking: {"type":"adaptive"}` | Relayed unchanged (`llm-gateway.clients.ThinkingHandling` `Relayed`): vLLM v0.27.1 ignores every `thinking` value. Effort is what turns the model's thinking on | `claude_code_adaptive_thinking_reaches_the_pod_unchanged`; scenario `claude-code-adaptive-thinking-reaches-the-target-unchanged` |
| Effort | `max` becomes `xhigh` on the messages wire, as `high` does; `low`, `medium` and `xhigh` reach the pod unchanged. Unmapped, `max` reaches the template and fails the turn | `w4_on_the_messages_wire_effort_max_reaches_the_target_as_xhigh`, `w4_low_medium_xhigh_and_any_other_effort_reach_the_target_unchanged`, `w4_on_the_chat_and_responses_wires_effort_max_passes_unchanged`; scenarios `w4-on-the-messages-wire-effort-max-reaches-the-target-as-xhigh`, `w4-on-the-chat-and-responses-wires-effort-max-passes-unchanged` |
| Structured output | `output_config.format` is relayed unchanged; the pinned vLLM serves it as structured output and ignores the deprecated top-level `output_format`, which is relayed unchanged too | `claude_code_structured_output_reaches_the_pod_with_its_format_unchanged`; scenario `claude-code-structured-output-format-reaches-the-target-unchanged` |
| Headers | `anthropic-version`, `anthropic-beta` and `x-claude-code-session-id` are dropped, as every client header is; vLLM v0.27.1 reads neither of the first two | `claude_code_a_tool_turn_and_its_tool_result_reach_the_pod_mapped_and_stream_back_unchanged`; scenario `claude-code-anthropic-headers-never-reach-the-target` |
| A tool turn and its result | The pod receives `/v1/messages` with `model` rewritten and effort mapped, every other body byte unchanged; the client receives the pod's stream byte for byte | `claude_code_a_tool_turn_and_its_tool_result_reach_the_pod_mapped_and_stream_back_unchanged` |

## What remains for the live record

None of these can be settled from the published sources; each needs a recorded session of Claude
Code 2.1.293 against a pod.

1. Whether Claude Code sends `output_config.effort` by default to a model id it does not
   recognise. The protocol page says such an id receives effort; the environment variable
   reference documents `CLAUDE_CODE_ALWAYS_ENABLE_EFFORT=1` to send effort "even when Claude Code
   does not recognize the model ID as effort-capable", which implies it may not. Without effort,
   the served template's default (`xhigh`) applies.
2. Whether Claude Code's stream idle watchdog trips without `ping` events. vLLM v0.27.1 writes
   none; thinking and text deltas normally flow throughout a turn, but a long pause before the
   first delta has nothing on the stream.
3. How Claude Code handles vLLM's OpenAI-shaped `400` body
   (`{"error":{"message":…,"type":…,"code":400}}`, not Anthropic's `{"type":"error",…}`): its
   retries that match on wording (an unrecognised tool type after `Input tag`, a rejected
   `output_config.effort`) may not fire.
4. Which Claude Code feature sends `output_config.format`, and with which `anthropic-beta` value.
   Neither is published; the fixture shapes it as a side request.
5. How Claude Code treats `405 method-not-allowed` on `POST /v1/messages/count_tokens`: the
   protocol page says it falls back to estimated counts when the endpoint is missing, which a
   `404` would say; the gateway answers `405` and relays nothing (`adversary_claude_code_wire.rs`).

Also unsettled, and outside the gateway: whether vLLM applies the structured-output grammar only
after the model's reasoning ends for this model, and the HTTP status a template `raise_exception`
reaches the client with (the source path reads as `500` with `internal_error`).
