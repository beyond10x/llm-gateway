---
format: aep.planning-md/3
id: story:token-and-cache-metrics
kind: story
status: draft
title: GET /metrics counts tokens by direction and cache tokens by kind per model
relations:
- decomposes: epic:gateway-features
- depends_on: story:usage-records
- informed_by: story:prometheus-metrics
revision: 1
---
## Outcome

`GET /metrics` counts the tokens each relayed model used, by direction, and the cache tokens by
kind, from the token counts the usage record carries.

## Acceptance

1. `llmgw_tokens_total{model,direction}` with `direction` `input` or `output`, and
   `llmgw_cache_tokens_total{model,kind}` with `kind` `read` or `write`, each summing the usage
   record's counters for relayed answers; a counter the target did not report adds nothing.
2. Registered at zero per relayed model; `docs/gateway.md` and `spec/domains/telemetry.yaml`
   declare both; a test pins each against fixture answers on all three wires.

## Depends on

`story:usage-records`, which puts the token counts into the record.
