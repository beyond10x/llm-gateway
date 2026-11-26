---
format: aep.planning-md/3
id: story:model-stop-route
kind: story
status: draft
title: The owner stops a model pod with POST /v1/models/{alias}/stop
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The owner can stop a model's pod: `POST /v1/models/{alias}/stop` beside the wake.

## Why

The owner, 2026-11-26: the Runpod side of the gateway "just comes with an api to start, stop the
model", so an operations view can wake and stop a pod. Today the gateway has a wake
(`POST /v1/models/{alias}/wake`) and stops a pod only by its idle timeout or at its own stop
(docs/gateway.md, routes).

## Acceptance

1. `POST /v1/models/{alias}/stop`, owner credential as the wake takes it: a model whose pod is
   `starting` or `ready` is terminated through the pool as the idle reap terminates it, and the
   answer names the phase after the call; a model already `stopped` answers the same with no
   provider call.
2. A call held on a pod that is stopped this way is answered `model-stopped`; a call relayed on it
   is cut as an upstream cut is today.
3. An unknown alias answers `model-unknown`; a model that is not a pod (hosted, route, Ollama,
   Bedrock) answers `path-unknown`, as its wake does.
4. Tests against the emulated Runpod for each phase and refusal; no test calls a real provider.
5. Declared in `spec/domains/gateway.yaml` with scenarios; `docs/gateway.md` lists the route.
