---
format: aep.planning-md/3
id: story:bedrock-invoke-effort
kind: story
status: implemented
title: A reasoning effort reaches an Anthropic model on InvokeModel
summary: output_config.effort reaches InvokeModel; reasoning_efforts lists what the model takes; an effort outside it is refused, never dropped
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/llm-gateway-cli/src/bedrock.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/bedrock.rs
- confidence: cited
  path: crates/llm-gateway-docs/src/status.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: website/docs/guides/serve-a-bedrock-model.md
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-20T23:45:33Z", actor: "agent:ai-agent-platform", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-20T23:45:33Z", actor: "agent:ai-agent-platform", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-20T23:56:45Z", actor: "agent:ai-agent-platform", revision: 5, decided_on: {"recorded":{"test_result":1},"asserted":{"test_result":1}}}
---
## Source

The operator, 2026-11-21: "you do need to make sure that we're not dropping any behavior. So if
LLM gateway is having that weird behavior of dropping the effort, make sure this isn't happening.
So we need to fix the LLM gateway then. You can just do that in this session."

A deployment that serves Claude on Bedrock through the gateway measured it (gateway 0.22.0, a
loopback Bedrock fixture): a messages client sent `thinking: {"type":"adaptive"}` and
`output_config: {"effort":"low"}` for an `anthropic-invoke` model; Bedrock received `thinking`
and no `output_config`, and the gateway logged `bedrock target: dropped a field it does not take
field="output_config.effort"`. The same body sent to `InvokeModel` directly carries the effort.

## Outcome

A reasoning effort reaches an Anthropic model on `InvokeModel` as llm's Messages encoder writes
it, `output_config.effort`, from every ingress wire. The model's `reasoning_efforts` (a
Bedrock-only key) lists the names it takes, `low`, `medium`, `high`, `xhigh` and `max` when
undeclared; an effort outside the list is refused `translation-unsupported`, never dropped. A
`converse` model still drops an effort, parallel tool calls and thinking for a model that is not
Anthropic's, each logged by name, for the reason `docs/gateway.md` gives.

## Acceptance

1. Effort `low`, `medium` and `high` with adaptive thinking on the messages wire reach the
   `InvokeModel` body of a Sonnet 4.6 model as `output_config.effort`, with `thinking` as sent,
   and no line logs the effort as dropped.
2. A forced tool on Haiku 5.5 with `thinking: disabled` and no effort reaches the body as it was
   sent: `thinking` disabled, `tool_choice` the named tool, no `output_config`.
3. An effort the model's `reasoning_efforts` does not hold is refused `translation-unsupported`
   and nothing reaches Bedrock.
4. `reasoning_efforts` on an endpoint or on a `converse` model, or naming an effort outside the
   five, refuses the start `config:value`.
5. `docs/gateway.md`, the deployment document reference, the Bedrock guide, the specification and
   `CHANGELOG.md` say so in the same change.
