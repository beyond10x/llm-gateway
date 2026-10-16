---
format: aep.planning-md/3
id: story:count-renders-effort
kind: story
status: draft
title: The output clamp's count renders the effort the relayed request renders
relations:
- decomposes: epic:gateway-features
- depends_on: story:max-tokens-clamp
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The output clamp's count renders the prompt the relayed request renders, so a clamped
`max_tokens` fits the window exactly whatever the model's chat template does with effort and
thinking.

## Why

vLLM v0.27.1's `count_tokens` ignores `output_config` (`serving.py:502-503`), so the count sets
no `reasoning_effort` and no `enable_thinking`, while the relayed request renders both into the
chat template. On a template that defaults thinking off the count is low and a clamped request
is refused again by vLLM; on Qwen3.8 at effort `low` or `medium` it is about 35 tokens high and
requests near the window are refused `context-exceeded` early
(`review-result:adversary-w08-max-tokens-clamp-pass-1`, findings 1 and 2).

## Acceptance

1. The count body carries the effort the relayed body names (or the declared one) as
   `chat_template_kwargs.reasoning_effort`, which the count request accepts
   (`protocol.py:254-269`), unless the body already names `chat_template_kwargs.reasoning_effort`.
2. A `wire_relay` case per effort spelling shows the count body the target is sent; the relayed
   body is unchanged from today.
3. The spec's Relay command and `docs/gateway.md` step 7 state what the count is sent.

## Scope (inferred)

`crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/src/body.rs`,
`crates/llm-gateway/tests/wire_relay.rs`, `spec/domains/gateway.yaml`, `docs/gateway.md`,
`CHANGELOG.md`.
