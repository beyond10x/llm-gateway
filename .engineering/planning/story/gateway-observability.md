---
format: aep.planning-md/3
id: story:gateway-observability
kind: story
status: draft
title: llm-gateway exposes its counters and logs
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:cold-start-hold
revision: 5
---
## Outcome

`/metrics` exposes process-wide and per-model, per-wire counters covering llmgw's 13 metric names, and logging follows `RUST_LOG`.

## Rows

R4, O1, O2, O3, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

- Rows R4, O1, O2 and O3 are each `covered` in the matrix, with the llm-gateway citation updated
  and a test that names the row id.
- `spec/domains/telemetry.yaml` no longer carries the `UNMAPPED` markers on series names or on
  where a record goes. Each is rewritten as `DECIDED`.
- The `Relay` conformance scenarios read one `UsageRecord` per request from the observation this
  story adds. They cover a relayed answer, a refusal and an `upstream-failed` call, and they check
  `disposition`, `refusal`, `status`, `response_bytes` and `duration_ms`. The token fields are
  absent.
- After those scenarios, each of the six series the record feeds has grown by exactly the
  records counted.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.

## Per-call record

The gateway makes one record per model call, `llm-gateway.telemetry.UsageRecord` in
`spec/domains/telemetry.yaml`. This story fills the fields that need no parsing: `model`, `wire`,
`disposition`, `refusal`, `status`, `response_bytes` and `duration_ms`. It also adds the
conformance observation the record is read from, and settles the specification's markers on the
series names and on where a record goes. The token fields and their series belong to
`story:usage-records`.

The record feeds six of llmgw's 13 series: `inference_requests_total`, `upstream_failures_total`,
`route_requests_total`, `route_refusals_total`, `route_upstream_status_failures_total` and
`route_response_bytes_total`. The other seven are counted where their events happen.
`instruction_views_total` counts `GET /` (row R1, `story:gateway-deployment`).
`endpoint_invalidations_total` counts target-source invalidations (row W7). `pod_starts_total`,
`pod_start_failures_total` and `pod_reaps_total` are counted in the pool.
`cold_start_wait_seconds_total` and `route_cold_start_holds_total` count the hold that
`story:cold-start-hold` adds, which this story depends on.

## Neighbours

These stories share files with this one:

- `story:gateway-deployment` adds `GET /` and `GET /v1/models` (rows R1, R5) to the route table
  in `crates/llm-gateway/src/server.rs`, where this story adds `GET /metrics` (R4).
- `story:cold-start-hold` hooks the relay path in `crates/llm-gateway/src/relay.rs` that
  `route_cold_start_holds_total` counts.

Do not schedule any of the three in one wave without an order between them
(`review-result:gateway-features-parallel-safety-round-1`).
