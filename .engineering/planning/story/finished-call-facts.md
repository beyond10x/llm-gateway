---
format: aep.planning-md/3
id: story:finished-call-facts
kind: story
status: draft
title: One finished-call value per call through a port of the gateway crate
relations:
- decomposes: epic:interaction-records
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

Every model call that was given a call id hands one finished-call value to the embedding, once,
after its usage record: the call id, the session hash and the harness that named it, the path,
the inbound wire, the route and upstream, attempts, queue wait, time to first token, the
declared-price cost, how the call ended, and its start and end instants.

## Why

These facts exist today in two places that do not meet: the usage record (`UsageRecords`, call id,
tokens, `cost_usd`) and the dashboard's finished call (`crates/llm-gateway/src/dashboard.rs`
486-511: attempts with their waits, `queue_wait_ms`, `ttft_ms`, the session). sessionspec's wire
lift takes them as one `GatewayFacts` value per call. A port of the gateway crate hands it, as
`UsageRecords` and `CallPricing` do (`crates/llm-gateway/src/metrics.rs:132`, `:139`).

## Acceptance

1. `spec/domains/telemetry.yaml` declares the finished-call value and its fields, each with its
   rule, and the scenarios hold it; `task spec-diff` names only additions.
2. A port in the gateway crate (`FinishedCalls`, declared on the relay as `UsageRecords` is)
   receives the value once per call that carries `x-llm-gateway-call`: relayed, translated,
   passthrough and refused calls; never for a shed request or one that failed authentication.
3. The session hash and harness are the ones the dashboard computes from the same headers
   (`x-claude-code-session-id`, `session-id`, `session_id`, `x-opencode-session`, else the user
   agent), by the same function; a test holds both equal for each header.
4. How the call ended is one of: complete, client disconnected, cut short by its upstream, ended
   in its wire's terminal error, refused (with the refusal code). The declared-price cost is
   present exactly when the usage record's `cost_usd` is.
5. A test per wire and per ending holds the value's fields against a fake upstream on loopback.
6. The value carries no request or answer bytes.
