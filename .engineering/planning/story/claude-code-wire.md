---
format: aep.planning-md/3
id: story:claude-code-wire
kind: story
status: draft
title: The messages wire answers what Claude Code sends, settled from the qualification record
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:client-qualification
- depends_on: story:public-model-listing
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/verification/client-qualification.md
- confidence: cited
  path: spec/domains/clients.yaml
- confidence: inferred
  path: spec/domains/gateway.yaml
revision: 7
---
## Outcome

The messages wire answers what Claude Code 2.1.293 sends, as recorded by
`story:client-qualification`: its startup probe, its `thinking` object and its headers, each either
handled by a change here or shown harmless by the record.

## Why

`docs/design/runpod-clients.md` § 1 lists three points where Claude Code's documented behaviour
meets this gateway and vLLM untested: `HEAD /api/hello` is answered `credential-absent` (D2),
`thinking: {"type":"adaptive"}` is relayed to vLLM unchanged (D3), and `anthropic-version` and
`anthropic-beta` are not forwarded (`crates/llm-gateway/src/relay.rs:582-587`).

## ESS first

`llm-gateway.clients.ThinkingHandling` is declared `PLANNED` in `spec/domains/clients.yaml`. This
story replaces its `UNMAPPED` marker with a `DECIDED` line citing the qualification record. A
mapping, if chosen, is declared on the `Relay` command in `spec/domains/gateway.yaml` beside the
effort mapping of row W4, before any code.

## Acceptance

1. `docs/verification/fixtures/claude-code-2.1.293-messages.json`, committed by
   `story:client-qualification`, is relayed by a `wire_relay` test: the fixture pod receives
   `/v1/messages` with `model` rewritten and effort mapped, every other byte unchanged unless
   criterion 3 maps `thinking`.
2. D2: when the record shows Claude Code fails on the `credential-absent` answer to
   `HEAD /api/hello`, the path is answered as an unauthenticated probe and a test pins it;
   otherwise `docs/gateway.md` states the answer and cites the record. Either way the refusal table
   tests pass.
3. D3: `ThinkingHandling` is `DECIDED` from the vLLM answer the record holds. With `Mapped`, a
   `Relay` scenario observes the rewritten value at the fixture pod; with `Relayed`, the record's
   successful turn is cited and no code changes.
4. The record's answer on headers is written in `docs/gateway.md` "The relay": either the two
   `anthropic-*` headers are forwarded, with a test, or the document states they are dropped and
   cites the record.

5. A Claude Code 2.1.293 session rerun after criteria 2 to 4, by the method of
   `story:client-qualification`, is appended to `docs/verification/client-qualification.md`: its
   capture shows an answer carrying a tool call and a following request carrying its result.

## Depends on

- `story:client-qualification`, whose record settles each criterion.
- `story:public-model-listing`: both change `docs/gateway.md` (its sentence at `:100` on the
  unauthenticated paths, which criterion 2 may extend), `spec/domains/clients.yaml` and
  `crates/llm-gateway/src/server.rs`.

## Neighbours

`crates/llm-gateway/src/relay.rs` and `docs/gateway.md` are also changed by
`story:hosted-endpoints`, `story:target-fallback`, `story:usage-records` and
`story:gateway-translation`. They come after this story: `story:hosted-endpoints` depends on it,
and the other three on `story:hosted-endpoints`. `story:gateway-observability` comes before it,
through `story:client-qualification`.
