---
format: aep.planning-md/3
id: story:a-call-starts-no-pod
kind: story
status: implemented
title: A model call starts no pod unless the document says so and names a daily cap
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T20:25:24Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T20:25:25Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T22:36:10Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":2,"verification":1}}}
---
## Outcome

A model call never starts a billed pod unless the deployment document says so and names a daily
spend cap. Without that, a pod is started by the owner's wake and by nothing else, and a call to a
model whose pod is not running is refused by a code of its own.

## Why

Today the first call to a `[models.<alias>]` model whose pod is stopped creates a pod
(`docs/gateway.md`, "The relay", step 7). A dispatcher that sends runs to a model route would
start billed pods as a side effect of a run. The operator, 2026-11-25: pods are started only by a
person until a daily spend cap for wakes is set; no call starts a pod unless that is enabled with
a cap.

## Acceptance

1. `[models.<alias>] start_on_request`, `true` or `false`, default `false`. A document that names
   `true` for a model whose provider declares no `daily_cap_usd` is refused at start as
   `config:value`, naming the model and the missing cap.
2. With `false`: a call to a model whose phase is `stopped` or `stopping` is refused `503` with
   the new code `model-stopped`, at once, without `retry-after`; no pod is created and the pool is
   asked for no start. Its message says that a request does not start the model's pod and that the
   owner wakes it. The usage record names the refusal, and its outcome is `refused`.
3. With `false`: a call to a model whose phase is `starting`, after the owner's wake, is held as a
   request is held today, and `model-cold-start` after the hold. A call to a `ready` model is
   relayed. A pod that fails while a call is held starts no replacement.
4. `POST /v1/models/{alias}/wake` starts the pod as today, under the spend check, whatever the key
   says.
5. With `true` and a cap: today's behaviour, call for call.
6. The Messages output clamp's count and the status routes start nothing, as before. Where
   `story:route-status-for-every-model` has landed, the status list's `callable` is `false` for a
   model a call would refuse `model-stopped`.
7. `CHANGELOG.md` names the changed default under `### Changed` as a break, with the key that
   restores the old behaviour. The specification (`llm-gateway.deployment`, `llm-gateway.gateway`)
   declares the key and the code; `task spec-diff` passes, with an acknowledgement if ESS classes
   the change breaking. Scenarios cover acceptance 2, 3 and 5. `docs/gateway.md`'s refusal table,
   the refusal reference and the deployment reference carry the code and the key.
8. Every test runs against `EmulatedRunpod`. No test reaches a provider or starts a pod.
