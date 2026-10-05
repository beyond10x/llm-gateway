---
format: aep.planning-md/3
id: story:wire-relay
kind: story
status: draft
title: llm-gateway relays the chat, responses and messages wires as llmgw does
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The three inference routes relay bytes and event streams to the selected pod within a 32 MiB body bound, rewrite `model`, map effort, refuse with typed errors, enforce each model's wire set and drop an endpoint after a failed request.

## Rows

R6, R7, R8, W1, W2, W3, W4, W5, W7, K11, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.
