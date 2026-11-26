---
format: aep.planning-md/3
id: story:route-kpi-view
kind: story
status: draft
title: One JSON view of per-route figures for every relayed model
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

One JSON view of per-route figures for every model the gateway relays, so an operations view can
show them without reading Prometheus text.

## Why

The owner, 2026-11-26: "generic model routes returning kpis, metrics, etc" to display beside the
gateway's dashboard. Today the figures exist only as `GET /metrics` (Prometheus text) and in the
dashboard's own snapshot.

## Acceptance

1. `GET /v1/kpis` (owner credential; `HEAD` too), format `llm-gateway.kpis/1`: one entry per
   relayed model in alias order with `kind`, `phase`, and over a stated window (`window_seconds`,
   the gateway's held ended calls at most): `requests`, `errors` by outcome, `input_tokens`,
   `output_tokens`, `latency_ms` p50 and p95 (end of answer), `ttfb_ms` p50 and p95,
   `output_tokens_per_second` (sum of output tokens over the sum of generation time), `cost_usd`
   (only from calls that have one) and, for a pod, `pod_hours` in the window.
2. Inbound side and outbound side: the same entry carries the counts by inbound wire and by
   upstream (provider and account), from the finished-call facts (`FinishedCall`).
3. Reading the view starts, wakes and probes nothing beyond what `GET /v1/status` does, and
   makes no usage record.
4. Tests with a fake clock and recorded finished calls for each figure, an empty window, and a
   model with no priced call (no `cost_usd`).
5. Declared in `spec/domains/gateway.yaml` with scenarios; `docs/gateway.md` gives the shape.
