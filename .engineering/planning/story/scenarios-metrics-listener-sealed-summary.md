---
format: aep.planning-md/3
id: story:scenarios-metrics-listener-sealed-summary
kind: story
status: active
title: Scenarios reach the metrics-only listener and the sealed reasoning summary
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: checks/conformance/src
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/telemetry.yaml
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:25Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:25Z", actor: "human:timo", revision: 4}
---
## Outcome

The conformance suite reaches the metrics-only listener and the first sealed reasoning item's
summary, so the two mutants that survive it today are killed by a scenario.

## Why

The hardening pass of 2026-10-30 (`docs/verification/spec-hardening-2026-10-30.md`) planted nine
implementation mutants. Two survived the 288-scenario suite and were killed only by crate tests:

- `m5-scrape-path`: dropping the exactly-`/metrics` path check on the `metrics_listen` listener
  (`crates/llm-gateway/src/server.rs:416`); no scenario reaches that listener
  (`spec/domains/telemetry.yaml:78`).
- `m9-sealed-item-summary`: negating the first sealed reasoning item's summary condition
  (`crates/llm-gateway-cli/src/translate.rs:1199`); no scenario observes it
  (`spec/domains/gateway.yaml:80`).

## Acceptance

Re-planting `m5-scrape-path` and `m9-sealed-item-summary` one at a time makes
`b10x-llm-gateway-conformance check` fail on a named scenario each; restored, `check` passes three
consecutive runs. Each new scenario is listed in `contracts/ess-inputs.yaml`.

## Scope (inferred)

`contracts/gateway/scenarios/`, `contracts/ess-inputs.yaml`, `checks/conformance/src/`,
`spec/domains/telemetry.yaml`, `spec/domains/gateway.yaml`.
