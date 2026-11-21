---
format: aep.planning-md/3
id: story:translated-answer-model-and-outcome
kind: story
status: implemented
title: A translated Messages answer names its alias, and an answer ending in an error is recorded as failed
summary: message_start and the message name the alias the client asked for; an error after the head is upstream_error in the record and counters
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/bedrock.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/src/translate.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/telemetry.yaml
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T04:45:29Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-21T04:45:29Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-21T05:16:38Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Source

The operator, 2026-11-21: "we need to fix the LLM gateway then. You can just do that in this session."

A deployment whose backend sends Anthropic Messages requests through the gateway to Bedrock measured
gateway 0.23.0 (llm 0.20.0) on `InvokeModel`, streamed, at most 64 output tokens:

- Every translated `message_start` carried no `model`: the translator builds its Messages stream as
  `llm_messages::IngressStream::new(id, None)`. A client that reads the model from `message_start`
  (the Messages wire always names it there) dropped the frame and with it the input and cache counts
  it carries.
- A stream the translator ended with the wire's `error` event after its head (a tool call cut off by
  `max_tokens`, and a Claude Haiku answer opening with signed thinking without text) was recorded
  `outcome=ok status=200`, so neither the usage record nor `llmgw_requests_total` showed the failure.

## Outcome

A translated Messages answer names the alias the client asked for as its model, in `message_start` and
in the non-streamed `message`, never the model Bedrock reported or the id it was sent to. A translated
answer whose head was written and which ended in its wire's terminal error is recorded with that
code's outcome (`upstream_error`) in the usage record and the counters; it stays `Relayed` with
status 200 and no refusal. `TranslatedAnswer::failed` carries the code to the relay.

## Acceptance

1. A streamed messages answer's `message_start` names the alias; the non-streamed `message` names the
   alias.
2. A streamed answer ending in an `error` event is recorded `outcome=upstream_error`,
   `disposition=Relayed`, `status=200`, no refusal, and counted under `upstream_error` in
   `llmgw_requests_total`.
3. A Claude answer opening with signed thinking without text reaches a messages client in place once
   llm carries story:empty-signed-reasoning; until the pin moves, the case is written and ignored, and
   the pull request quotes a local run against that llm branch.
4. `docs/gateway.md`, `spec/domains/telemetry.yaml` and `CHANGELOG.md` say so.

## Not covered

A tool call cut off by `max_tokens` still ends in an `error` event: llm refuses its arguments, which
are not JSON, and the neutral turn has no form for them. Moving the llm pin is a follow-up step after
the llm release.
