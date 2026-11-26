---
format: aep.planning-md/3
id: epic:interaction-records
kind: epic
status: draft
title: Each finished model call becomes session records in a local store sessionspec reads
relations:
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

llm-gateway turns each finished model call into sessionspec's session records and appends them to
a local store, so the same session view that shows a harness transcript shows the gateway's
traffic, with what only the wire sees: route, upstream, attempts, queue wait, time to first token,
declared-price cost and how a call ended.

## Why

The owner accepted on 2026-11-26 one shared model for agent sessions
(`sessionspec/docs/interaction-records.md`, every default). sessionspec owns the domain
`sessionspec.session` and publishes the crate `sessionspec-records` with the wire lift
(`wire::lift_call`) and a non-blocking appender. The gateway owns its side: which relay point hands
which facts, the bytes the lift reads, the store's place in its document, and the privacy line.

## Scope

- In: one finished-call value through a port of the gateway crate; a bounded copy of a call's
  request and answer bytes for the embedding, off unless declared; the binary's adapter that lifts
  and appends through `sessionspec-records` to a SQLite file its document names.
- Out: the dashboard's session view (sessionspec's `session-view` component, embedded later); an
  Eventlog or entity-runtime store for a gateway of many users; message text in any store (the
  lift keeps digests and sizes).

## Constraints

- The gateway crate declares no dependency; `sessionspec-records` is linked by
  `b10x-llm-gateway-cli` only, by git tag.
- No relay waits on the store: the append never blocks, and a full queue counts a drop.
- Specification first: every new value is declared in `spec/domains/telemetry.yaml`.
