---
format: aep.planning-md/3
id: story:ollama-route
kind: story
status: implemented
title: A model on a local Ollama server, with its status and never a pull
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:route-status-for-every-model
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T21:53:17Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T21:53:18Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-26T01:05:38Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Outcome

The deployment document names a model on a local Ollama server, and the gateway serves it on its
chat, responses and messages paths beside its other models, with a status read from Ollama's own
API. The gateway never pulls a model.

## Why

The operator, 2026-11-25: "that gateway should also have support for Ollama, basically having a
route to a local Ollama thing." A hosted model can already name Ollama's OpenAI-compatible base
URL, and then the gateway cannot say whether the model is pulled or loaded.

## Acceptance

1. A model table of kind Ollama: `base_url`, default `http://127.0.0.1:11434`, and the Ollama
   model name the alias is sent as. The key names are the implementor's and are published in the
   deployment reference. A `base_url` on a host that is not loopback follows the rule of a hosted
   model.
2. A call on the chat wire is relayed to Ollama's OpenAI-compatible chat path; calls on the
   responses and messages wires are served too, relayed where Ollama serves the wire and
   translated where it does not. Which is which is written in `docs/gateway.md`.
3. Its status, on `GET /v1/models/{alias}/status` and in `GET /v1/status` of
   `story:route-status-for-every-model`: `unreachable` with `sub_phase` `no-answer` when the
   server does not answer; `unreachable` with `sub_phase` `model-not-pulled` when `GET /api/tags`
   does not list the model; `stopped` with `sub_phase` `not-loaded` when it is pulled and
   `GET /api/ps` does not list it; `ready` when `GET /api/ps` lists it.
4. A call to a model that is not pulled is refused `503` with a code of its own,
   `model-not-pulled`, whose message names the alias. A model that is pulled and not loaded is
   `callable`: the first call loads it. The gateway sends no pull request, and no request to Ollama
   that changes what the server stores: a test fails on any request to `/api/pull`,
   `/api/create`, `/api/copy`, `/api/delete` or `/api/push`.
5. The tokens Ollama reports reach the usage record as a hosted model's do.
6. The specification, scenarios, `docs/gateway.md`, the deployment reference, a guide page and
   `CHANGELOG.md` carry the kind, the status values and the code. The guide names a model to
   try and the disk it takes.
7. Every test uses a fake Ollama on loopback. No test starts Ollama or pulls a model.

## Depends on

`story:route-status-for-every-model`: the phase `unreachable` and the status list.
