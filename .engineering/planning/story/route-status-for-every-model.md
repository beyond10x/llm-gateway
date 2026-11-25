---
format: aep.planning-md/3
id: story:route-status-for-every-model
kind: story
status: active
title: Every served model has a status, and one list holds them all
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T20:25:20Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T20:25:20Z", actor: "human:operator", revision: 3}
---
## Outcome

The owner reads, for every model the gateway serves, whether a call would be answered now, without
sending a model call and without starting anything. One list holds all of them.

## Why

A dispatcher that sends a run to a named model route checks the route before the run. Today
`GET /v1/models/{alias}/status` exists for a model on a gateway-owned pod only; a hosted model and
a route of the routes document answer `404 path-unknown` (`docs/gateway.md`, "The lifecycle
routes", step 4), and `GET /ready` describes the process, never an upstream. The operator's goal
of 2026-11-25: the gateway serves several named model routes at once, with a route list and a
status per route.

## Acceptance

1. `GET /v1/models/{alias}/status` for a hosted model or a route of the routes document answers
   `200` with a `ModelStatus`. Its `phase` is `ready` when the endpoint answered
   `GET <base path>/models` with a 2xx status, and the new phase `unreachable` otherwise, with
   `sub_phase` saying why: `no-answer` (no connection, a certificate that does not verify, no
   answer head inside the probe's deadline), `credential-refused` (401 or 403) or
   `status-<code>` (any other status).
2. For an `auth_kind = "client"` model the probe carries no credential, and a 401 or 403 answer is
   `ready`: the endpoint answered, and the gateway holds no credential that could be refused.
3. The probe sends the model's own credential and static headers and nothing of the caller's: not
   the owner secret, not a client's credential. It starts nothing, makes no usage record and
   counts no route series. It has a deadline of its own, published with the bounds of
   `docs/gateway.md`. A result is reused for a published time, so repeated reads send one probe
   per model in that time, and two probes of one model never run at once.
4. `POST /v1/models/{alias}/wake` for these models stays `path-unknown`. A Bedrock model's status
   stays `path-unknown`. A pod model's status and wake are unchanged.
5. `GET /v1/status` answers `{"format":"llm-gateway.status/1","models":[…]}`, one entry per
   relayed model in alias order: `model`, `kind` (`pod`, `hosted`, `route` or `bedrock`), `wires`,
   the keys of its `ModelStatus`, and `callable`: `true` when a call sent now would be relayed or
   held, `false` when it would be refused. A model that has no status carries no `phase` and no
   `callable`. It takes the
   credential `GET /v1/launch` takes. With three hosted models whose endpoints accept a connection
   and never answer, it is answered within one probe deadline plus one second.
6. The specification declares the phase and the list (`llm-gateway.gateway`), scenarios under
   `contracts/gateway/scenarios/` cover acceptance 1, 2 and 5, and `docs/gateway.md`, the
   deployment reference and `CHANGELOG.md` say what is new.
7. Every test uses a fake upstream on loopback. No test reaches a provider.

## Not in this story

- A probe that runs without being asked (a background check). The dashboard's reader of a pod's
  `/metrics` (`crates/llm-gateway-cli/src/vllm_stats.rs`) stays as it is.
- A route kind of its own for a local Ollama server: `story:ollama-route`.
