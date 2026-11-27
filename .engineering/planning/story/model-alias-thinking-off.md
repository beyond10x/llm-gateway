---
format: aep.planning-md/3
id: story:model-alias-thinking-off
kind: story
status: implemented
title: An alias of a Runpod model reaches vLLM with thinking off
relations:
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-27T13:29:53Z", actor: "human:operator", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-27T13:29:53Z", actor: "human:operator", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-27T13:57:24Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":2,"review_outcome":1}}}
---
## Problem

A consumer runs Claude Code against a Runpod model served by vLLM v0.27.1 with Qwen3.8. Claude
Code sends `output_config.effort` on every messages request (`high` by default, never `none`);
the gateway maps `high` to `xhigh`, and vLLM turns the chat template's thinking on for every
effort it is given. Structured steps then reason up to their output cap and answer no JSON, and
no client setting switches thinking off.

## Outcome

`[aliases.<alias>]` names a Runpod model and `effort = "none"`. The alias is served by the
model's pod; on the messages wire every request to it reaches vLLM with `output_config.effort`
`null` and `chat_template_kwargs.enable_thinking` `false`, whatever the client sent. The model's
own name is unchanged. Bad aliases are refused at start.

## Acceptance

Tests in `crates/llm-gateway` and `crates/llm-gateway-cli`: an absent, `low`, `high` and `max`
effort and a client `chat_template_kwargs` all reach the stub pod with thinking off; the alias
and its model share one target; the model relays byte for byte as before; each config refusal.
One conformance scenario; floors 435 of 435.

Decision: asked by a consumer as gap 5 on 2026-11-27; assigned by the host operator.
