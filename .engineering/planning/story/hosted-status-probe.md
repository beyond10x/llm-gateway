---
format: aep.planning-md/3
id: story:hosted-status-probe
kind: story
status: draft
title: A hosted model names its status probe, for an endpoint with no model list
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

A hosted model whose endpoint answers no model list can still report its status: its declaration
names the probe, and `GET /v1/status` calls it `ready` and callable when that probe answers 2xx.

## Why

Measured on the owner's host, 2026-11-26, with 0.25.0: a hosted model on a local Messages proxy
that adds an account credential itself answers model calls and `POST /v1/messages/count_tokens`
(200), but 404 on `GET /v1/models`, `/models`, `/ready` and `/health`. The status probe of a hosted
model is `GET <base path>/models`, so the model is listed `unreachable` `status-404`, callable
false, and Runbook's readiness check (which reads `callable`) refuses a route on it although every
call would be answered.

## Acceptance

1. `[hosted.<alias>] status_probe`: `models` (default, today's probe) or `count-tokens` (a
   `POST <base path>/messages/count_tokens` with a one-word message, Messages wire only; refused
   at start as `config:value` on a model without the Messages wire). Declared in
   `spec/domains/deployment.yaml`.
2. With `count-tokens`, a 2xx answer is `ready`, any other the same `unreachable` sub-phases as
   today; the probe keeps today's bounds (5 s, reused 10 s, one at a time).
3. A test per answer against a fake endpoint on loopback, and one that the default is unchanged.
4. The probe makes no model call: a test fails on any request other than the declared probe.
