---
format: aep.planning-md/3
id: story:prometheus-metrics
kind: story
status: draft
title: Prometheus scrapes latency and token counters per model without the owner credential
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- supersedes: story:request-duration-metric
- supersedes: story:token-and-cache-metrics
- depends_on: story:usage-records
revision: 1
---
## Outcome

A Prometheus server scrapes the gateway's request latency and token use per model without holding
the owner credential. `GET /metrics` on the serving listener stays owner-only, as today; an
optional metrics listener serves the same text and nothing else.

## Why

A monitoring stack should not hold the credential that can make paid model calls. Today the
counters sit behind the owner secret (`crates/llm-gateway/src/server.rs:73`), and they hold
neither latency nor tokens. This story takes over `story:request-duration-metric` and
`story:token-and-cache-metrics` and adds the scrape path.

## The rule

- **Latency.** A histogram `llmgw_request_duration_seconds{model}` with fixed buckets, published in
  `docs/gateway.md`, observes each usage record of a relayed model, from the end of the request
  head to the last byte written (the record's milliseconds).
- **Tokens.** `llmgw_tokens_total{model,direction}` (`input`, `output`) and
  `llmgw_cache_tokens_total{model,kind}` (`read`, `write`) sum the usage record's counts. A count
  the target did not report adds nothing.
- **Series.** Every relayed or hosted model is registered at zero when the relay is composed.
  Labels are model aliases from the document and fixed words; no label or value carries a
  credential, a prompt, a completion or a document's content.
- **Scrape path.** `[gateway] metrics_listen = "<address>:<port>"`, optional and absent by default,
  binds a second listener that answers `GET /metrics` and nothing else (any other method or path
  is `404`, and no model route is reachable). It takes no owner credential. If
  `metrics_bearer_file` names a trusted file (the endpoint-credential rules), a scrape must send
  that token as `authorization: Bearer`; without it the listener is open, and the guides say to
  bind it only on an address a cluster network policy guards. The listener drains and stops with
  the gateway. The address must differ from `listen`.

## Acceptance

1. Spec first: `spec/domains/telemetry.yaml` declares the histogram and both counters, and
   `spec/domains/deployment.yaml` declares `metrics_listen` and `metrics_bearer_file`. Both
   validate with the newest `ess`, and conformance scenarios observe the series after relayed,
   refused and failed requests.
2. A test reads the histogram and token series after fixture answers on all three wires,
   including an answer that reports no token count.
3. A test shows the metrics listener answers `GET /metrics` without a credential, refuses every
   other path, and refuses a missing or wrong scrape token when `metrics_bearer_file` is set. The
   owner-guarded `/metrics` is unchanged.
4. No rendered series contains a key, the owner token, or request or answer text, checked by a
   test that relays a request whose body and key are distinctive strings.
5. `docs/gateway.md`, the deployment-document reference, the guide "Scrape the counters", the
   Kubernetes guide (scrape port and network policy), README, `AGENTS.md`, CHANGELOG and the
   status page name the series and the listener.
