---
title: Scrape the counters
sidebar_position: 2
description: Read llmgw's 13 counters, the gateway's spend-refusal counter and the latency and outcomes of every model from GET /metrics, scrape them without the owner secret, and read the usage events from standard error.
lede: GET /metrics serves llmgw's 13 counters under their llmgw names behind the owner credential, so a scrape configured for llmgw keeps working, one counter of the gateway's own, and a latency histogram and an outcome counter per model; an optional second listener serves the same text to a Prometheus server that holds no owner secret.
source: docs/gateway.md (Counters and usage records), crates/llm-gateway/src/metrics.rs, crates/llm-gateway/tests/observability.rs, crates/llm-gateway/tests/prometheus_metrics.rs, crates/llm-gateway-cli/tests/observability.rs, crates/llm-gateway-cli/tests/binary.rs; the output below is pasted from a run of the binary on the getting-started document
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
# HELP llmgw_pod_start_spend_refusals_total Pod starts refused because they would pass a spend ceiling or the spend is unknown.
# TYPE llmgw_pod_start_spend_refusals_total counter
llmgw_pod_start_spend_refusals_total 0
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
# HELP llmgw_requests_total Model calls per model, by how they ended.
# TYPE llmgw_requests_total counter
llmgw_requests_total{model="small",outcome="ok"} 0
llmgw_requests_total{model="small",outcome="upstream_error"} 0
llmgw_requests_total{model="small",outcome="timeout"} 0
llmgw_requests_total{model="small",outcome="refused"} 1
# HELP llmgw_request_duration_seconds Seconds from the end of a model call's request head to the last byte written, per model.
# TYPE llmgw_request_duration_seconds histogram
llmgw_request_duration_seconds_bucket{model="small",le="0.05"} 1
llmgw_request_duration_seconds_bucket{model="small",le="0.1"} 1
llmgw_request_duration_seconds_bucket{model="small",le="0.25"} 1
llmgw_request_duration_seconds_bucket{model="small",le="0.5"} 1
llmgw_request_duration_seconds_bucket{model="small",le="1"} 1
llmgw_request_duration_seconds_bucket{model="small",le="2.5"} 1
llmgw_request_duration_seconds_bucket{model="small",le="5"} 1
llmgw_request_duration_seconds_bucket{model="small",le="10"} 1
llmgw_request_duration_seconds_bucket{model="small",le="30"} 1
llmgw_request_duration_seconds_bucket{model="small",le="60"} 1
llmgw_request_duration_seconds_bucket{model="small",le="120"} 1
llmgw_request_duration_seconds_bucket{model="small",le="300"} 1
llmgw_request_duration_seconds_bucket{model="small",le="+Inf"} 1
llmgw_request_duration_seconds_sum{model="small"} 0
llmgw_request_duration_seconds_count{model="small"} 1
```

The answer is `content-type: text/plain; version=0.0.4; charset=utf-8`. Without the credential
it is `credential-absent`, and before readiness `unavailable`, as for route inspection.

## What the series count

- Every (model, wire) pair a model declares is registered at zero when the gateway starts, so an
  uncalled route reads `0` instead of being absent.
- `llmgw_inference_requests_total` counts usage records: one per model call that passed owner
  authentication, relayed or not. An unauthenticated or shed request makes none.
- Every model is registered at zero in `llmgw_requests_total` and
  `llmgw_request_duration_seconds` too: all four outcomes, every bucket, the sum and the count.
  Each usage record naming a model feeds both, whatever wire it came on.
- `outcome` is `ok` when an answer was relayed, whatever its status; `upstream_error` when the
  target failed, refused the call or closed without an answer; `timeout` when the target's read
  timeout or a start deadline passed; `refused` when the gateway refused the call before it
  reached a target, as `target-unavailable` above. The table from each refusal code to its
  outcome is in
  [`docs/gateway.md`](https://github.com/beyond10x/llm-gateway/blob/main/docs/gateway.md#call-outcomes);
  `model-cold-start` is `timeout`.
- The histogram observes the record's duration, from the end of the request head to the last
  byte written, in seconds. Its buckets are `0.05`, `0.1`, `0.25`, `0.5`, `1`, `2.5`, `5`, `10`,
  `30`, `60`, `120` and `300` seconds and `+Inf`, cumulative as the Prometheus text format writes
  them.
- Labels are model aliases from the deployment document and fixed words. No series carries a
  key, the owner secret, a prompt or an answer.
- The pod series are counted by the Runpod composition. In the shipped binary, which starts no
  pod, they stay `0`.

## Scrape without the owner secret

A Prometheus server should not hold the secret that can make paid model calls. Set
`metrics_listen` in the deployment document and the gateway binds a second listener that
serves the same text at `GET /metrics` and nothing else:

```toml
listen = "0.0.0.0:8080"
owner_secret_file = "owner-secret"
metrics_listen = "0.0.0.0:9090"
metrics_bearer_file = "scrape-token"
```

```bash
curl -s -H "Authorization: Bearer $(cat scrape-token)" http://127.0.0.1:9090/metrics
```

- Any other method or path on it is `404 path-unknown`: no model route, probe or inspection path
  is reachable there.
- With `metrics_bearer_file`, a scrape must send that token. None is `401 credential-absent`, and
  another token, the owner secret included, `401 credential-rejected`. The file follows the
  rules of a [credential file](../reference/deployment-document.md#credential-files) and holds
  at least 32 bytes.
- Without `metrics_bearer_file` the listener is open. Bind it only on an address a network policy
  guards, as [Run on Kubernetes](run-on-kubernetes.md#scrape-the-counters) does.
- It answers before the gateway is ready and while it drains, and it stops with the gateway.
- `metrics_listen` must differ from `listen`. The owner-guarded `/metrics` on `listen` is
  unchanged.

A Prometheus job for it:

```yaml
scrape_configs:
  - job_name: llm-gateway
    authorization:
      credentials_file: /etc/prometheus/llm-gateway-scrape-token
    static_configs:
      - targets: ["llm-gateway.example.internal:9090"]
```

## Usage events on standard error

The binary writes each usage record as one `tracing` event at `info`, which is the default level:

```text
2026-10-08T21:19:58.427410Z  INFO usage: usage record model="small" wire=chat disposition=Refused refusal=target-unavailable status=503 response_bytes=0 duration_ms=0
```

`RUST_LOG=warn` or `RUST_LOG=off` silences them; the lines the binary writes itself
(`listening on`, `refused`, `stopped by`) stay. Token counts are always absent from a record for
now; carrying the counts the target reported is planned.
