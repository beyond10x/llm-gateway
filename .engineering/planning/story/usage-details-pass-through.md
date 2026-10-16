---
format: aep.planning-md/3
id: story:usage-details-pass-through
kind: story
status: implemented
title: Usage details, cached tokens included, reach the client unchanged on the chat and messages wires
relations:
- decomposes: epic:gateway-features
- depends_on: story:max-tokens-clamp
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-15T17:42:26Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-15T17:42:27Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-16T01:43:53Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Outcome

The usage block of a target's answer, cache details and any field the gateway does not know
included, reaches the client unchanged on the chat and messages wires, streamed or not; held by
scenarios, so a later change that parses answers cannot drop a field unnoticed.

## Why

A vLLM started with `--enable-prompt-tokens-details` reports
`usage.prompt_tokens_details.cached_tokens` on the chat wire and `cache_read_input_tokens` on the
messages wire. The relay already returns the answer's decoded body bytes unchanged
(`docs/gateway.md` "The relay" step 9); nothing pins that for usage fields.

## Acceptance

1. Fixtures, authored from vLLM v0.27.1's usage models and marked authored: a chat answer, not
   streamed and streamed (final chunk with `usage`, `stream_options.include_usage`), carrying
   `prompt_tokens_details.cached_tokens` and an unknown usage field; a messages answer, not
   streamed and streamed (`message_start` and `message_delta` usage), carrying
   `cache_read_input_tokens`, `cache_creation_input_tokens` and an unknown usage field.
2. A `Relay` scenario per wire and a `wire_relay` test per fixture: the client receives the
   fixture byte for byte.
3. No code change is expected; if one is needed, it is a finding recorded here, not a silent fix.
   The usage record's token counters stay absent: mapping them is `story:usage-records`.

## Scope (inferred)

`crates/llm-gateway/tests/fixtures/`, `crates/llm-gateway/tests/wire_relay.rs`,
`contracts/gateway/scenarios/`, `contracts/ess-inputs.yaml`, `contracts/suite.json`,
`contracts/baseline.json`, `AGENTS.md` (scenario count), `CHANGELOG.md`.
