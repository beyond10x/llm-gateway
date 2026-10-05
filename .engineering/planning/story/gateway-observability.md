---
format: aep.planning-md/3
id: story:gateway-observability
kind: story
status: draft
title: llm-gateway exposes its counters and logs
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

`/metrics` exposes process-wide and per-model, per-wire counters covering llmgw's 13 metric names, and logging follows `RUST_LOG`.

## Rows

R4, O1, O2, O3, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.
