---
format: aep.planning-md/3
id: story:llm-0-7-0-pin
kind: story
status: active
title: The gateway translates with llm 0.7.0, and Claude Code's and Codex's turns are no longer known limits
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/translation.rs
- confidence: cited
  path: crates/llm-runpod/Cargo.toml
- confidence: cited
  path: docs/gateway.md
- confidence: inferred
  path: website/docs/guides/translate-between-wires.md
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T10:09:01Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-17T10:09:01Z", actor: "human:timo", revision: 5}
---
## Outcome

`b10x-llm-gateway-cli` takes llm's client crates and `b10x-llm-models` at tag `0.7.0`. On a
translated route, Claude Code's first turn and Codex's first and second turns, which llm 0.6.0's
ingress refused field by field, are carried, translated or refused by name as llm 0.7.0's
"Client request fields" rule says; the known-limit tests in
`crates/llm-gateway-cli/tests/translation.rs` (lines 371-512 at 6f5f6b4) state the new answer.

## Why

llm 0.7.0 (https://github.com/beyond10x/llm/releases/tag/0.7.0) widens its Messages and Responses
ingress to the fields these clients send and stops passing over unknown nested keys. The gateway
pins 0.6.0, so a translated route still refuses those clients. This clears
`dependency-blocker:llm-0-7-0-ingress`.

## Acceptance

1. Every llm dependency of `crates/llm-gateway-cli` (and `crates/llm-runpod` if it moves with
   it) names tag `0.7.0`; `Cargo.lock` holds no 0.6.0 llm crate and no `aws-lc` crate.
2. Each known-limit test is replaced by a test of what 0.7.0 does with that turn: carried and
   relayed where llm carries it, refused naming the field where llm still refuses it
   (`thinking.display`, `context_management`, `output_config.format`, ...). The gateway's own
   refusal codes are unchanged.
3. Opaque reasoning state on a translated route is still refused `translation-opaque-state`
   (that rule moves in `story:cross-wire-reasoning-state`), and a test says so for Codex's
   second turn.
4. The workspace gate is green; `docs/gateway.md`, the translation guide, AGENTS.md, README and
   CHANGELOG name llm 0.7.0 and the turns now carried.
