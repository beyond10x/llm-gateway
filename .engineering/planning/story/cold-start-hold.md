---
format: aep.planning-md/3
id: story:cold-start-hold
kind: story
status: draft
title: A request for a cold model waits for its pod within a hold budget
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
revision: 1
---
## Outcome

A request for a model whose pod is not serving is held within a request hold budget separate from the startup deadline, answered 503 with Retry-After past it; the reaper runs on a timer and orphans are swept at startup.

## Rows

L6, W6, K28, L13, L15, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.
