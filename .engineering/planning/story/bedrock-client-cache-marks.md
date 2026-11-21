---
format: aep.planning-md/3
id: story:bedrock-client-cache-marks
kind: story
status: active
title: llm 0.24.0, and a messages client's cache marks reach InvokeModel as the client placed them
summary: the llm pin moves to 0.24.0; the client's cache_control marks arrive at the same blocks and none is added to a request that carries its own
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: Cargo.lock
- confidence: cited
  path: README.md
- confidence: cited
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: cited
  path: crates/llm-gateway-cli/src/bedrock.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/bedrock.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/translation.rs
- confidence: cited
  path: crates/llm-gateway-docs/src/status.rs
- confidence: cited
  path: crates/llm-runpod/Cargo.toml
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: website/docs/guides/translate-between-wires.md
revision: 18
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T05:50:04Z", actor: "human:timo", revision: 17}
- {from: "proposed", to: "active", at: "2026-11-21T05:50:04Z", actor: "human:timo", revision: 18}
---
## Source

The operator, 2026-11-21: "you do need to make sure that we're not dropping any behavior. So if
LLM gateway is having that weird behavior of dropping the effort, make sure this isn't happening.
So we need to fix the LLM gateway then. You can just do that in this session." Then, 2026-11-21,
"A" to: tag `llm` and release `llm-gateway` 0.24.0, moving the llm pin.

The same measurement as `story:bedrock-invoke-effort`: in every request the gateway put
`cache_control: {"type":"ephemeral"}` on the last message's last markable block, also when the
client sent no mark; a client's own mark on a system block was kept. A cache write costs more
than an uncached read, so a mark the client did not send changes what the client pays.

The mark is llm's: llm 0.20.0 `crates/llm-messages/src/codec.rs` `mark_rolling_breakpoint`
(called from `project`) places a rolling breakpoint on the tail and one on the joined system
block, and its ingress decoder keeps only the marks' lifetime (`docs/messages.md`: "the caller's
mark positions are not reproduced"). `llm_bedrock`'s `InvokeModel` encoder takes that body.

llm 0.24.0 (https://github.com/beyond10x/llm/releases/tag/0.24.0) carries the placement:
`Hints::cache_marks` holds where a Messages client put its marks, and the Messages and
`InvokeModel` encoders place exactly those and none of their own; a request without a mark keeps
the system and rolling breakpoints. The gateway gets it by moving its llm pin.

## Outcome

Every llm dependency of the workspace names tag `0.24.0`. A messages client's request to an
`anthropic-invoke` model that carries at least one `cache_control` mark reaches Bedrock with
exactly the client's marks, at the same blocks with the same `ttl`, and none added. A request with
no mark keeps llm's automatic breakpoints. A chat or responses client, which cannot express a
mark, keeps the automatic ones.

The pin move also runs the case `story:translated-answer-model-and-outcome` left ignored: signed
thinking without text reaches a messages client as a thinking block before the answer, with no
`error` event.

## Acceptance

1. Every llm dependency of `crates/llm-gateway-cli` and `crates/llm-runpod` names tag `0.24.0`;
   `Cargo.lock` holds no other llm version and no `aws-lc` crate. `Capabilities` is built with
   `Capabilities::text` and its `with_*` setters.
2. A request whose only mark is on its system block: exactly one mark arrives, on that block.
3. A request with no mark: llm's two automatic breakpoints arrive (the system block and the last
   message's last block).
4. Marks on a content block and on a tool arrive on the same block and tool with the same `ttl`.
5. `bedrock_signed_thinking_without_text_reaches_a_messages_client_in_place` runs and passes.
6. `docs/gateway.md`, AGENTS.md, README, the status page and the site name llm 0.24.0, and
   CHANGELOG has the change under Unreleased.

## Not in this story

A model setting that turns the automatic breakpoints off: llm 0.24.0 has none, so a request
with no mark always gets them.
