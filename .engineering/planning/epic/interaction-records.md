---
format: aep.planning-md/3
id: epic:interaction-records
kind: epic
status: draft
title: Each finished model call becomes session records in a local store sessionspec reads
relations:
- serves: vision:portable-model-inference
revision: 2
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

- In: one finished-call value and, when declared, a bounded copy of the call's bytes, through one
  port of the gateway crate (story:finished-call-facts); the binary's adapter that lifts and
  appends through `sessionspec-records` to a SQLite file its document names
  (story:interaction-record-store).
- Out:
  - the dashboard's session view: sessionspec's `session-view` component, embedded later;
  - the live channels `sessions` and `session_events`: `sessionspec serve` serves them over the
    same SQLite file; the gateway serving them itself is a later story, when its dashboard embeds
    the view;
  - an Eventlog or entity-runtime store for a gateway of many users, or one in Kubernetes;
  - message text in any store: the lift keeps digests and sizes.

## Constraints

- The gateway crate declares no dependency; `sessionspec-records` is linked by
  `b10x-llm-gateway-cli` only, by git tag.
- No relay waits on the store: the append never blocks, and a full queue counts a drop.
- Specification first: the value and the port in `spec/domains/telemetry.yaml`, the document table
  in `spec/domains/deployment.yaml`. The specification names sessionspec's records by a
  `BOUNDARY:` comment beside the finished-call value and copies no part of the domain. The call
  record gains no key field: the call id is the pointer from a session record to it (design
  section 6).
- The binary holds a bounded count of sessions' lift state. A session dropped from it starts a new
  agent at its next call; the bound and that effect are in the guide.
- Count-tokens calls (`POST /v1/messages/count_tokens`) are no model turn and make no record.
- New tests land as modules of each crate's one `tests/main.rs` (story:test-step-within-budget):
  these stories start after that story is on the wave branch.

## Done when

story:finished-call-facts and story:interaction-record-store are implemented and released, and a
gateway on the owner's host with `[interaction_records]` writes a store that `sessionspec serve`
shows: one session with its agents, tool calls and the gateway's facts on each model turn.
