---
format: aep.planning-md/3
id: story:usage-records
kind: story
status: draft
title: Every model call yields a usage record with the token counts the target reported
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:gateway-observability
- depends_on: story:target-fallback
revision: 4
---
## Outcome

For every model call it relays, the gateway's usage record (`llm-gateway.telemetry.UsageRecord`)
carries the token counters the target reported, along with `reported_model`. A counter the target
did not report stays absent. The record's other fields need no parsing, and
`story:gateway-observability` owns them.

## Why

`vision:portable-model-inference` moves objective O6, attributable usage and cost. Claude Code and
Codex reach the gateway directly and are not llm clients, so llm's caller-side cost ledger never
sees their calls. Envoy, Kong, LiteLLM and Cloudflare all record token usage per request
(`epic:gateway-features`).

## Blocked

This story waits on `decision-blocker:usage-from-responses`, because reading usage requires
parsing the target's answers, which the relay does not do today. If the blocker is cleared with
option B or C, this story is archived.

## ESS first

`llm-gateway.telemetry.UsageRecord` in `spec/domains/telemetry.yaml` is marked `PLANNED`. This
story adds the token fields to the observation that `story:gateway-observability` creates for the
record, and adds the scenarios.

## Acceptance

Each case is a `Relay` conformance scenario over loopback fixture targets, and reads the record
from the observation. None makes a paid call.

1. On each of the three wires, a fixture that reports usage yields a record with exactly those
   counters and the model name the fixture reported. This holds both for a streamed answer and
   for one that is not streamed.
2. A fixture that reports no usage and no model yields a record with every counter absent and
   `reported_model` absent. A reported zero is recorded as zero.
3. A call that ends `upstream-failed` yields a record with every counter absent.
4. A call whose position 0 answers 503 and whose position 1 reports usage yields one record,
   carrying position 1's counters only.
5. The client receives the target's answer byte for byte, so the existing `Relay` scenarios in
   `contracts/gateway/scenarios/` still pass.
6. This story adds one token series per counter, per model and wire. Their names follow the scheme
   `story:gateway-observability` settles in `spec/domains/telemetry.yaml`. After case 1, each
   series has grown by exactly the reported count.

## Depends on

- `story:gateway-observability`, which creates the record, its observation and the naming scheme
  for series.
- `story:target-fallback`. Both stories change `relay::serve` in `crates/llm-gateway/src/relay.rs`,
  and one record per call has to account for the attempt loop that story adds (case 4).

## Scope (inferred)

`crates/llm-gateway/src/relay.rs` (the answer path in `relay::serve`), a parsing port that
`crates/llm-gateway-cli` implements, `spec/domains/telemetry.yaml`, `contracts/gateway/scenarios`,
`checks/conformance`, `docs/gateway.md`.
