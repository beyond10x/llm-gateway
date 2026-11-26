---
format: aep.planning-md/3
id: story:hosted-status-probe
kind: story
status: active
title: A hosted model names its status probe, for an endpoint with no model list
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T06:29:16Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-26T06:29:17Z", actor: "human:operator", revision: 4}
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
   `spec/domains/deployment.yaml`. The probe never asks for a generation: a test fails on any
   request to the fake other than the declared probe.
2. With `count-tokens`, a 2xx answer is `ready`, any other the same `unreachable` sub-phases as
   today; the probe keeps today's bounds (5 s, reused 10 s, one at a time).
3. A test per answer against a fake endpoint on loopback, and one that the default is unchanged.
4. The status reports what the gateway's own record shows about which clients the model serves:
   when, among the model's last relayed calls, those from clients whose `user-agent` names Claude
   Code were answered 2xx and those from other clients were answered `429` with an error of type
   `rate_limit_error`, `GET /v1/status` keeps the model `ready` and adds the reason
   `serves-claude-code-clients-only` (shown on the dashboard as "reachable, serves Claude Code
   clients only"), and a call from another client is still relayed unchanged. The gateway never
   alters a client's headers or body to pass that check; no test imitates Claude Code against a
   real endpoint. A test against the fake for each side, and one where the record holds too few
   calls to say anything (no reason added).
