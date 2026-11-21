---
format: aep.planning-md/3
id: story:bedrock-client-cache-marks
kind: story
status: draft
title: A messages client's cache marks reach InvokeModel as the client placed them
summary: the client's cache_control marks arrive at the same blocks; none is added to a request that carries its own
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Source

The operator, 2026-11-21: "you do need to make sure that we're not dropping any behavior. So if
LLM gateway is having that weird behavior of dropping the effort, make sure this isn't happening.
So we need to fix the LLM gateway then. You can just do that in this session."

The same measurement as `story:bedrock-invoke-effort`: in every request the gateway put
`cache_control: {"type":"ephemeral"}` on the last message's last markable block, also when the
client sent no mark; a client's own mark on a system block was kept. A cache write costs more
than an uncached read, so a mark the client did not send changes what the client pays.

The mark is llm's: llm 0.20.0 `crates/llm-messages/src/codec.rs` `mark_rolling_breakpoint`
(called from `project`) places a rolling breakpoint on the tail and one on the joined system
block, and its ingress decoder keeps only the marks' lifetime (`docs/messages.md`: "the caller's
mark positions are not reproduced"). `llm_bedrock`'s `InvokeModel` encoder takes that body.

## Outcome

A messages client's request to an `anthropic-invoke` model that carries at least one
`cache_control` mark reaches Bedrock with exactly the client's marks, at the same blocks with the
same `ttl`, and none added. A request with no mark keeps llm's automatic breakpoints, unless the
model's configuration turns them off. A chat or responses client, which cannot express a mark,
keeps the automatic ones.

## Acceptance

1. A request whose only mark is on its system block: exactly one mark arrives, on that block.
2. A request with no mark: llm's documented automatic breakpoints arrive, and none when the model
   turns them off.
3. Marks on content blocks arrive on the same blocks with the same `ttl`.

## Waits for

An llm release whose neutral turn carries a client's mark placement and whose Messages and
`InvokeModel` encoders reproduce it (`upstream-blocker:llm-client-cache-marks`), and the gateway's
llm pin moved to it.
