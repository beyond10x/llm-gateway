---
title: Scrape the counters
sidebar_position: 2
description: Read llmgw's 13 counters from GET /metrics and the usage events from standard error.
lede: GET /metrics serves llmgw's 13 counters under their llmgw names behind the owner credential, so a scrape configured for llmgw keeps working.
source: docs/gateway.md (Counters and usage records), crates/llm-gateway/src/metrics.rs, crates/llm-gateway/tests/observability.rs, crates/llm-gateway-cli/tests/observability.rs; the output below is pasted from a run of the binary on the getting-started document
---

# Scrape the counters

Start the gateway as in [Getting started](../getting-started.md). The run below came after that
page's requests: three answers of `GET /` and one model call refused `target-unavailable`.

```bash
curl -s -H "Authorization: Bearer $(cat owner-secret)" http://127.0.0.1:8080/metrics
```

```text
# HELP llmgw_inference_requests_total Model calls the gateway was asked to relay, one per usage record.
# TYPE llmgw_inference_requests_total counter
llmgw_inference_requests_total 1
# HELP llmgw_upstream_failures_total Model calls whose target could not be reached or closed without an answer.
# TYPE llmgw_upstream_failures_total counter
llmgw_upstream_failures_total 0
# HELP llmgw_instruction_views_total Setup instruction pages served at GET /.
# TYPE llmgw_instruction_views_total counter
llmgw_instruction_views_total 3
# HELP llmgw_pod_starts_total Pod creates the pool submitted.
# TYPE llmgw_pod_starts_total counter
llmgw_pod_starts_total 0
# HELP llmgw_pod_start_failures_total Pods that could not be started or failed while starting.
# TYPE llmgw_pod_start_failures_total counter
llmgw_pod_start_failures_total 0
# HELP llmgw_pod_reaps_total Deployments and pods the cleanup pass stopped.
# TYPE llmgw_pod_reaps_total counter
llmgw_pod_reaps_total 0
# HELP llmgw_endpoint_invalidations_total Targets the gateway reported failed to its target source.
# TYPE llmgw_endpoint_invalidations_total counter
llmgw_endpoint_invalidations_total 0
# HELP llmgw_cold_start_wait_seconds_total Seconds requests spent held for a starting target.
# TYPE llmgw_cold_start_wait_seconds_total counter
llmgw_cold_start_wait_seconds_total 0
# HELP llmgw_route_requests_total Model calls per model and wire.
# TYPE llmgw_route_requests_total counter
llmgw_route_requests_total{model="small",wire="chat"} 1
llmgw_route_requests_total{model="small",wire="responses"} 0
llmgw_route_requests_total{model="small",wire="messages"} 0
# HELP llmgw_route_refusals_total Model calls refused or left without a target answer, per model and wire.
# TYPE llmgw_route_refusals_total counter
llmgw_route_refusals_total{model="small",wire="chat"} 1
llmgw_route_refusals_total{model="small",wire="responses"} 0
llmgw_route_refusals_total{model="small",wire="messages"} 0
# HELP llmgw_route_upstream_status_failures_total Model calls whose target answered a status outside 2xx, per model and wire.
# TYPE llmgw_route_upstream_status_failures_total counter
llmgw_route_upstream_status_failures_total{model="small",wire="chat"} 0
llmgw_route_upstream_status_failures_total{model="small",wire="responses"} 0
llmgw_route_upstream_status_failures_total{model="small",wire="messages"} 0
# HELP llmgw_route_cold_start_holds_total Model calls held for a starting target, per model and wire.
# TYPE llmgw_route_cold_start_holds_total counter
llmgw_route_cold_start_holds_total{model="small",wire="chat"} 0
llmgw_route_cold_start_holds_total{model="small",wire="responses"} 0
llmgw_route_cold_start_holds_total{model="small",wire="messages"} 0
# HELP llmgw_route_response_bytes_total Body bytes of target answers relayed to clients, per model and wire.
# TYPE llmgw_route_response_bytes_total counter
llmgw_route_response_bytes_total{model="small",wire="chat"} 0
llmgw_route_response_bytes_total{model="small",wire="responses"} 0
llmgw_route_response_bytes_total{model="small",wire="messages"} 0
```

The answer is `content-type: text/plain; version=0.0.4; charset=utf-8`. Without the credential
it is `credential-absent`, and before readiness `unavailable`, as for route inspection.

## What the series count

- Every (model, wire) pair a model declares is registered at zero when the gateway starts, so an
  uncalled route reads `0` instead of being absent.
- `llmgw_inference_requests_total` counts usage records: one per model call that passed owner
  authentication, relayed or not. An unauthenticated or shed request makes none.
- The pod series are counted by the Runpod composition. In the shipped binary, which starts no
  pod, they stay `0`.

## Usage events on standard error

The binary writes each usage record as one `tracing` event at `info`, which is the default level:

```text
2026-10-08T21:19:58.427410Z  INFO usage: usage record model="small" wire=chat disposition=Refused refusal=target-unavailable status=503 response_bytes=0 duration_ms=0
```

`RUST_LOG=warn` or `RUST_LOG=off` silences them; the lines the binary writes itself
(`listening on`, `refused`, `stopped by`) stay. Token counts are always absent from a record for
now; carrying the counts the target reported is planned.
