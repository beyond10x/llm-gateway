---
format: aep.planning-md/3
id: story:gateway-deployment
kind: story
status: draft
title: llm-gateway ships a container image and the proven model profiles
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

A container image, the two proven model profiles as model declarations, the setup page at `/`, an OpenAI-shaped `/v1/models` with wires, the Runpod API key and the pod's vLLM key read from files, and the inference call to the pod.

## Rows

D1, D3, R1, R5, K7, B8, B9, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.
