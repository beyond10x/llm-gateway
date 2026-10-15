---
format: aep.planning-md/3
id: story:request-duration-metric
kind: story
status: draft
title: GET /metrics reports relayed request duration per model
relations:
- decomposes: epic:gateway-features
revision: 1
---
## Outcome

`GET /metrics` reports how long relayed requests take per model, so latency can be watched without
reading logs.

## Acceptance

1. A histogram `llmgw_request_duration_seconds{model}` (Prometheus text, fixed buckets documented
   in `docs/gateway.md`) observes every usage record naming a relayed model, from the end of the
   request head to the last byte written (the record's existing milliseconds).
2. Every relayed model is registered at zero when the relay is composed, as the route series are.
3. `spec/domains/telemetry.yaml` declares it; a test reads the rendered series after relayed,
   refused and failed requests.
