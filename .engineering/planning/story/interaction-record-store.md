---
format: aep.planning-md/3
id: story:interaction-record-store
kind: story
status: draft
title: The binary lifts each finished call into session records in a SQLite file
relations:
- decomposes: epic:interaction-records
- serves: vision:portable-model-inference
- depends_on: story:finished-call-facts
- depends_on: story:call-bytes-tap
revision: 1
---
## Outcome

With `[interaction_records] path = "<file>"` in its document, the binary lifts every finished call
into sessionspec's session records and appends them to that SQLite file, which `sessionspec serve`
and the session view read. Without the table it does nothing new.

## Why

The owner accepted the shared session model on 2026-11-26. `sessionspec-records`
(`sessionspec/docs/interaction-records.md`, section "Built") offers `wire::lift_call` and a
non-blocking `append::Appender`; the gateway's adapter feeds them from the finished-call value
(story:finished-call-facts) and the call's bytes (story:call-bytes-tap).

## Acceptance

1. `b10x-llm-gateway-cli` links `sessionspec-records` by a git tag; the gateway crate gains no
   dependency.
2. `[interaction_records]`: `path` (required; a file the binary may create, refused at start by name
   when its directory is missing), `queue` (calls held for the writer, default 1024), `call_bytes`
   (the copy's bound, default 8 MiB). An unknown key is `config:schema`.
3. One `WireSession` per session hash, at most a bounded count held, the least recently used
   dropped first; the bound is in the document.
4. The append never blocks a relay: a full queue drops the call and counts it in
   `llmgw_interaction_records_dropped_total`; a test fills the queue and holds the relay's answer
   time unchanged.
5. A test against a fake upstream on loopback runs a two-agent session (a spawn tool call and the
   child's first message) through the binary and reads the store with `sessionspec-records`'
   own reader: both agents, the parent link, the tool calls, and the gateway facts on each model
   turn.
6. No message text in the store: the test greps the SQLite file for every prompt and tool input it
   sent and finds none.
7. A guide page names the table, the file, and how `sessionspec serve` reads it.
