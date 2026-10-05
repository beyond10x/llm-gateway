---
format: aep.planning-md/3
id: story:gateway-binary
kind: story
status: draft
title: llm-gateway runs as one configured binary with a clean shutdown
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
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
