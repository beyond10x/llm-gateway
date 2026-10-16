---
format: aep.planning-md/3
id: story:claude-code-wire
kind: story
status: implemented
title: The messages wire answers what Claude Code sends, settled from the qualification record
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:public-model-listing
- depends_on: story:hosted-endpoints
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: inferred
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/fixtures/claude-code
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/llmgw-capability-matrix.md
- confidence: cited
  path: docs/verification/2026-10-15-claude-code-wire.md
- confidence: cited
  path: spec/domains/clients.yaml
- confidence: inferred
  path: spec/domains/gateway.yaml
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-10-15T09:40:44Z", actor: "human:timo", revision: 11, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-15T09:40:44Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "active", to: "implemented", at: "2026-10-16T01:43:52Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Outcome

The messages wire answers what Claude Code 2.1.293 sends to a Qwen3.8-27B-FP8 model behind the
pinned vLLM (`vllm/vllm-openai:v0.27.1`, `--enable-auto-tool-choice --tool-call-parser
qwen3_coder`): its startup probe, its adaptive `thinking` object, its effort, its structured-output
request and its headers, each either handled by a change here or shown harmless by a cited source.
It is settled offline, from request and response fixtures authored from Claude Code's published
gateway protocol and from vLLM v0.27.1's source, never captured from a pod: no pod is started and
nothing is paid for.

## Why

`docs/design/runpod-clients.md` § 1 lists three points where Claude Code's documented behaviour
meets this gateway and vLLM untested: `HEAD /api/hello` is answered `credential-absent` (D2),
`thinking: {"type":"adaptive"}` is relayed to vLLM unchanged (D3), and `anthropic-version` and
`anthropic-beta` are not forwarded (`crates/llm-gateway/src/relay.rs`, the head the gateway writes).
The live record that was to settle them (`story:client-qualification`) waits on an unfunded
account; the published sources settle each point well enough to ship, and the live record checks
the result later.

What the sources say (read 2026-10-15):

- vLLM v0.27.1 `vllm/entrypoints/anthropic/protocol.py:118-178`: the messages request model has no
  `thinking` field and drops unknown keys, so every `thinking` value is ignored, never refused.
- vLLM v0.27.1 `anthropic/serving.py:505-514`: `output_config.effort` becomes `reasoning_effort`,
  which forces `enable_thinking` (`openai/chat_completion/protocol.py:564-573`);
  `output_config.format` `{"type":"json_schema","schema":…}` becomes `response_format`, vLLM's
  structured outputs. The deprecated top-level `output_format` is ignored.
- The Qwen3.8-27B-FP8 chat template accepts the efforts `xhigh`, `medium` and `low` and raises on
  any other; Claude Code sends `low|medium|high|xhigh|max`. The gateway maps `high` today
  (matrix row W4); `max` reaches the template and fails the turn.
- vLLM v0.27.1 reads neither `anthropic-version` nor `anthropic-beta`, and has no `/api/hello`
  route.
- https://code.claude.com/docs/en/llm-gateway-protocol: `HEAD /api/hello` is a connection-warming
  probe "a gateway can reject without breaking anything"; Claude Code sends `thinking:
  {"type":"adaptive"}` to a model id it does not recognise; `output_config.format` is sent with its
  own beta header and is switched off by `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS`.

## ESS first

`llm-gateway.clients.ThinkingHandling` is declared `PLANNED` with an `UNMAPPED` marker in
`spec/domains/clients.yaml`. This story replaces the marker with a `DECIDED` line: `Relayed`, citing
vLLM v0.27.1's request model. The effort mapping on the `Relay` command in
`spec/domains/gateway.yaml` (row W4) gains `max`, and a `Relay` scenario observes it, before any
code. The structured-output relay is declared on the same command as relayed unchanged.

## Acceptance

1. Fixtures under `crates/llm-gateway/tests/fixtures/claude-code/`, each headed by where it was
   derived from (a document URL or a vLLM v0.27.1 `path:line`) and marked authored, not captured:
   a Claude Code request carrying `?beta=true`, `anthropic-version`, `anthropic-beta`, adaptive
   `thinking`, `output_config.effort`, tools and `cache_control`; the vLLM stream answering it with
   a thinking block, a `signature_delta` and a `tool_use` block ending in `stop_reason` `tool_use`;
   the follow-up request carrying the `tool_result`; and a request with `output_config.format`. A
   `wire_relay` test relays each request to the fixture pod: the pod receives `/v1/messages` with
   `model` rewritten and effort mapped, every other byte unchanged; the client receives the
   fixture stream byte for byte.
2. D2: `HEAD /api/hello` keeps its `credential-absent` answer; `docs/gateway.md` states that answer
   and cites the published protocol page. The refusal table tests pass.
3. D3: `ThinkingHandling` is `DECIDED` `Relayed`; a `Relay` scenario observes adaptive `thinking`
   unchanged at the fixture pod; `docs/gateway.md` "The relay" says why, citing vLLM v0.27.1.
4. Effort: on the messages wire `output_config.effort` (and `chat_template_kwargs.reasoning_effort`)
   equal to `max` becomes `xhigh`, as `high` does; a `Relay` scenario and a `wire_relay` test pin
   it; `low`, `medium` and `xhigh` reach the pod unchanged; `docs/gateway.md` step 6 and the
   matrix row W4 say so.
5. Structured output: a request with `output_config.format` reaches the pod with that object
   unchanged (a test), and `docs/gateway.md` states that the pinned vLLM serves it as structured
   output and ignores the deprecated top-level `output_format`.
6. Headers: `docs/gateway.md` "The relay" states that `anthropic-version` and `anthropic-beta` are
   dropped, as every client header is, and cites that vLLM v0.27.1 reads neither.
7. A dated record `docs/verification/2026-10-15-claude-code-wire.md` lists each source read and
   what remains for the live record: whether Claude Code sends effort by default to an unknown id,
   whether its stream idle watchdog trips without `ping` events (vLLM sends none), how it handles
   vLLM's OpenAI-style 400 body, and which feature sends `output_config.format`.

The live Claude Code session with a tool call and its result is `story:client-qualification`'s,
which checks this story against a pod once the account is funded.

## Depends on

- `story:public-model-listing` (implemented): both change `docs/gateway.md` (the sentence on the
  unauthenticated paths), `spec/domains/clients.yaml` and `crates/llm-gateway/src/server.rs`.

## Neighbours

`crates/llm-gateway/src/relay.rs`, `body.rs` and `docs/gateway.md` are also changed by
`story:target-fallback`, `story:usage-records` and `story:gateway-translation`, all later.
