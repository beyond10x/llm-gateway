---
format: aep.planning-md/3
id: story:max-tokens-clamp
kind: story
status: active
title: A messages request never asks for more output than the context window has room for
relations:
- decomposes: epic:client-access
- depends_on: story:claude-code-wire
- depends_on: story:effort-default
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: inferred
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway/src/body.rs
- confidence: cited
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: cited
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: website/docs/reference/refusals.md
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-15T12:58:58Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-15T12:58:58Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":2}}}
---
## Outcome

A messages-wire request never asks the pod for more output than its context window has room for:
the gateway counts the request's input tokens at the target and lowers `max_tokens` to the room
left, and a request with no room left is refused with an answer Claude Code reads as a prompt
that is too long, instead of a vLLM 500.

## Why

A live test of 0.7.0 by a consumer (vLLM v0.27.1, `--max-model-len 65536`, Claude Code 2.1.300):
the first turn used its whole 32k `max_tokens`; the continuation's input plus `max_tokens` passed
the 64k window and vLLM answered 500. vLLM v0.27.1 serves `POST /v1/messages/count_tokens`
(`entrypoints/anthropic/api_router.py:95-133`), which answers `{"input_tokens": n}` for a
messages body.

## ESS first

The `Relay` command in `spec/domains/gateway.yaml` declares the count, the clamp, which window
applies, and the new refusal (`llm-gateway.gateway.Refusal` `ContextExceeded`, its code, status
400 and message). Validated and regenerated before code.

## Acceptance

1. The window `w` of a model: a Runpod model's `max_model_len` (what `vllm serve` is started
   with); a hosted model's `context_window`. A hosted model that declares no `context_window` has
   none: its requests are relayed with no count and no clamp (a test).
2. On the messages wire, for a model with a window, after a target is acquired and before the
   request is relayed, the gateway sends `POST /v1/messages/count_tokens` to the same target with
   the rewritten body and the relay's credential header. With `input_tokens = n`: a `max_tokens`
   above `w - n` is rewritten in place to `w - n`; one at or below is unchanged. Tests at
   `w - n - 1`, `w - n` and `w - n + 1` against the fixture pod, which answers `count_tokens` from
   a fixture marked as authored from vLLM v0.27.1's source.
3. `w - n < 1` is refused as `context-exceeded` (400) without relaying; the refusal body is the
   gateway's envelope and its message contains "prompt is too long", the phrase Claude Code's
   compaction reacts to (cite the source used). The refusal table in `docs/gateway.md`, the
   generated refusal reference and `RefusalCode::ALL` gain it.
4. A count that fails (connection, non-2xx, a body without a non-negative integer
   `input_tokens`) relays the request unchanged, as before this story, a test each; no metric
   series is added (the counters stay llmgw's 13). The count runs only after `acquire` handed out
   a target, so it never wakes a pod the relay would not have woken.
5. A body without a top-level integer `max_tokens` is relayed without a count. The chat and
   responses wires are unchanged (a test).
6. `docs/gateway.md` "The relay" gains the step; CHANGELOG has the line.

## Depends on

`story:claude-code-wire` and `story:effort-default`: all three change `relay.rs`, `body.rs`,
`tests/wire_relay.rs`, the `Relay` command and scenarios, "The relay" in `docs/gateway.md` and
`CHANGELOG.md`, and the count runs on the body `story:effort-default` may have given an effort.
This story comes last.
