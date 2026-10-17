---
format: aep.planning-md/3
id: story:prometheus-metrics
kind: story
status: implemented
title: Prometheus scrapes latency and call outcomes per model without the owner credential
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- supersedes: story:request-duration-metric
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: contracts/gateway/scenarios
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/telemetry.yaml
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T10:09:01Z", actor: "human:timo", revision: 8}
- {from: "proposed", to: "active", at: "2026-10-17T10:09:02Z", actor: "human:timo", revision: 9}
- {from: "active", to: "implemented", at: "2026-10-17T19:00:39Z", actor: "human:timo", revision: 11, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A Prometheus server scrapes the gateway's request latency and call outcomes per model without
holding the owner credential. `GET /metrics` on the serving listener stays owner-only, as today;
an optional metrics listener serves the same text and nothing else.

## Why

A monitoring stack should not hold the credential that can make paid model calls. Today the
counters sit behind the owner secret (`crates/llm-gateway/src/server.rs:717-729`), and they hold
neither latency nor outcomes. This story takes over `story:request-duration-metric` and adds the
outcome counter and the scrape path.

The token series stay in `story:token-and-cache-metrics`: the gateway reads no token count from an
answer today (`crates/llm-gateway/src/server.rs:447-452` sets every token field of the usage record
to `None`), and reading them is `story:usage-records`, which waits on llm usage readers for all
three wires (`dependency-blocker:llm-usage-decoders`; llm 0.7.0 does not export them).

## The rule

- **Latency.** A histogram `llmgw_request_duration_seconds{model}` with fixed buckets, published in
  `docs/gateway.md`, observes each usage record of a relayed model, from the end of the request
  head to the last byte written (the record's `duration_ms`).
- **Outcomes.** `llmgw_requests_total{model,outcome}` counts each usage record of a model call,
  with `outcome` one of `ok` (an answer relayed), `upstream_error` (the target failed, refused
  the call or closed without an answer), `timeout` (the target's read timeout or a start deadline
  passed; a translated request's transport timeout reaches the gateway as an upstream failure and
  counts `upstream_error`) and `refused` (the gateway refused the call before reaching a target: a refusal code
  such as `spend-ceiling` or `target-unavailable`). The mapping from every refusal code and
  disposition to one outcome is a table in `docs/gateway.md`.
- **Series.** Every relayed or hosted model is registered at zero when the relay is composed.
  Labels are model aliases from the document and fixed words; no label or value carries a
  credential, a prompt, a completion or a document's content.
- **Scrape path.** `metrics_listen = "<address>:<port>"`, a top-level key beside `listen` (the
  document has no `[gateway]` table), optional and absent by default,
  binds a second listener that answers `GET /metrics` and nothing else (any other method or path
  is `404`, and no model route is reachable). It takes no owner credential. If
  the top-level `metrics_bearer_file` names a trusted file (the endpoint-credential rules), a scrape must send
  that token as `authorization: Bearer`; without it the listener is open, and the guides say to
  bind it only on an address a cluster network policy guards. The listener drains and stops with
  the gateway. The address must differ from `listen`.

## Acceptance

1. Spec first: `spec/domains/telemetry.yaml` declares the histogram and the outcome counter, and
   `spec/domains/deployment.yaml` declares `metrics_listen` and `metrics_bearer_file`. Both
   validate with the newest `ess`, and conformance scenarios observe the series after relayed,
   refused and failed requests.
2. A test reads the histogram after fixture answers on all three wires and checks each bucket
   boundary flips at its published number.
3. A test drives one call to each outcome (an answer, a target that fails, a target that times
   out, a refused call) and reads `llmgw_requests_total{model,outcome}` advance by one for exactly
   that outcome; a test pins the refusal-code-to-outcome table against `RefusalCode::ALL`.
4. A test shows the metrics listener answers `GET /metrics` without a credential, refuses every
   other path, and refuses a missing or wrong scrape token when `metrics_bearer_file` is set. The
   owner-guarded `/metrics` is unchanged.
5. No rendered series contains a key, the owner token, or request or answer text, checked by a
   test that relays a request whose body and key are distinctive strings.
6. `docs/gateway.md`, the deployment-document reference, the guide "Scrape the counters", the
   Kubernetes guide (scrape port and network policy), README, `AGENTS.md`, CHANGELOG and the
   status page name the series and the listener.
