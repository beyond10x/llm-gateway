---
format: aep.planning-md/3
id: story:effort-default
kind: story
status: implemented
title: A Claude Code turn reaches the pod with an effort the template accepts, the model's declared one when it names none
relations:
- decomposes: epic:client-access
- depends_on: story:claude-code-wire
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-15T12:58:57Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-15T12:58:57Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-16T01:43:52Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":5,"verification":1}}}
---
## Outcome

A messages-wire request that carries no effort, sent to a hosted model that declares a
`reasoning_effort`, reaches the endpoint with that effort, so a hand-started server's thinking
length is the operator's choice rather than the chat template's `xhigh` default. The mapping of an
effort the request does carry (`high` and `max` to `xhigh`) is `story:claude-code-wire`'s, and this
story relies on it unchanged.

## Why

A live test of 0.7.0 by a consumer (static upstream, vLLM v0.27.1, Qwen3.8-27B-FP8,
`--max-model-len 65536`, Claude Code 2.1.300) observed: the request reached vLLM with no effort, so
the server default applied, and the first turn thought for its whole 32k `max_tokens` (226 s).
Directly against vLLM, effort `high` answers 500 "Unexpected reasoning effort" and `medium` works.
The Qwen3.8-27B-FP8 template accepts only `xhigh`, `medium` and `low` and defaults to `xhigh`
when no effort is given; vLLM v0.27.1 maps `output_config.effort` to `reasoning_effort` and forces
`enable_thinking` with it (`anthropic/serving.py:513-514`, `openai/chat_completion/protocol.py:564-573`).
A Runpod model already declares `reasoning_effort`, which the gateway passes to the pod it starts
as the server's default chat-template argument (`crates/llm-runpod/src/request.rs:71-77`), so a
Runpod model needs nothing here. A `[hosted.*]` model is reached at a server the operator started
by hand, whose defaults the gateway does not set: the request is the only place to say it.

## ESS first

`spec/domains/deployment.yaml` gains the hosted declaration's `reasoning_effort`
(`low|medium|xhigh`, absent by default), and the `Relay` command in `spec/domains/gateway.yaml`
declares the insertion of a model's declared effort when the request names none. Validated and
regenerated before code.

## Acceptance

1. `[hosted.<alias>] reasoning_effort = "low" | "medium" | "xhigh"` loads; any other value,
   `high` and `max` included, is `config:value`; absent by default. The deployment-document
   reference lists the key.
2. On the messages wire, a request that carries no effort (neither `output_config.effort` nor
   `chat_template_kwargs.reasoning_effort`), sent to a hosted model that
   declares `reasoning_effort = v`, reaches the fixture endpoint with `output_config.effort` equal
   to `v` exactly (an existing `output_config` object gains the key; a missing one is added),
   every other byte unchanged. A `Relay` scenario and a `wire_relay` test pin it, using
   `story:claude-code-wire`'s Claude Code request fixture with its effort removed, once per value.
3. Unchanged relay, a test each: a hosted model that declares no `reasoning_effort`; a Runpod
   model, whatever it declares; a request that carries either effort key (it keeps
   `story:claude-code-wire`'s mapping, not the declared value); the chat and responses wires.
4. `docs/gateway.md` "The relay" step 6 and the deployment-document page state the rule;
   CHANGELOG has the line.

## Depends on

`story:claude-code-wire`: same `body.rs`, `Relay` command, fixtures and `docs/gateway.md` step 6.
`story:max-tokens-clamp` depends on this story, because both change `crates/llm-gateway/src/body.rs`
(the span-recording `Analysis`/`rewrite` pair), `relay.rs`, `tests/wire_relay.rs`, the `Relay`
command, the `Relay` scenarios, "The relay" in `docs/gateway.md` and `CHANGELOG.md`: they run one
after the other, this one first.
