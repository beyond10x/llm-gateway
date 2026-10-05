---
format: aep.planning-md/3
id: story:gateway-binary
kind: story
status: active
title: llm-gateway runs as one configured binary with a clean shutdown
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: README.md
- confidence: inferred
  path: crates/llm-gateway
- confidence: inferred
  path: docs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:33:47Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-05T10:33:47Z", actor: "human:timo", revision: 3}
---
## Outcome

A `b10x-llm-gateway` binary (clap derive) loads a closed TOML configuration through a trusted-file reader, prints help and version, and shuts down on SIGINT and SIGTERM.

## Rows

C1, C2, C4, C5, K29, K30, D2, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.
