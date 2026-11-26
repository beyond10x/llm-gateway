---
format: aep.planning-md/3
id: story:stats-events
kind: story
status: draft
title: Aggregated figures emitted to an event bus on an interval
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The gateway emits aggregated figures to an event bus on an interval, so an operations view can
show both sides of the gateway as they change.

## Why

The owner, 2026-11-26: the gateway "could also be configured with a port for an eventbus and
would then emit its events (not every single token, but it would aggregate/collect some data,
and then periodically emit these stats". The bus contract (outward events, patterns kept apart
from the transport) is being designed in the ESS project; this story waits for it.

## Acceptance

1. Blocked until the ESS outward-event contract is published: the event's shape and its
   transport binding are taken from it, not invented here.
2. `[events]` in the deployment document: the bus endpoint and `interval_seconds`; without the
   table nothing is emitted and nothing connects.
3. Each interval emits one event per relayed model with the figures of `llm-gateway.kpis/1`
   (story:route-kpi-view) for that interval; never one event per token or per call.
4. A bus that is down or slow never delays or fails a model call: events are dropped and counted
   in a metric.
5. Tests against a fake bus on loopback for each line.
