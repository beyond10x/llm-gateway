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
- depends_on: story:test-step-within-budget
- depends_on: story:cli-test-port-race
revision: 2
---
## Outcome

With `[interaction_records] path = "<file>"` in its document, the binary lifts every finished call
into sessionspec's session records and appends them to that SQLite file, which `sessionspec serve`
and the session view read. Without the table the binary makes no copy, starts no writer and
creates no file.

## Why

The owner accepted the shared session model on 2026-11-26. `sessionspec-records`
(`sessionspec/docs/interaction-records.md`, section "Built") offers `wire::lift_call` and a
non-blocking `append::Appender`; the binary's adapter feeds them from the port of
story:finished-call-facts.

## Design

- The adapter implements `FinishedCalls` in `b10x-llm-gateway-cli`. It holds one `WireSession` per
  session hash, at most `sessions_held`, the least recently used dropped first; a dropped session's
  next call starts a new agent.
- `Copied` bytes are lifted and appended. `OverBound` is not lifted: it is counted in
  `llmgw_interaction_records_skipped_total`. A full queue is counted in
  `llmgw_interaction_records_dropped_total`. Both through the gateway crate's public count method.
- A refused call is lifted as an incomplete turn with the reason `refused`.

## Acceptance

1. `b10x-llm-gateway-cli` links `sessionspec-records` by a git tag; the gateway crate gains no
   dependency.
2. `spec/domains/deployment.yaml` declares `[interaction_records]`: `path` (required; refused at
   start by name when its directory is missing), `queue` (default 1024), `call_bytes` (default
   8 MiB), `sessions_held` (default 4096). An unknown key is `config:schema`.
3. Without the table: the relay is composed without `with_call_bytes` and without the adapter, no
   writer thread runs, and no file is created; a test holds all three.
4. With the writer held stopped, the test sends twice `queue` calls: every answer completes, and
   the drop count equals the calls beyond `queue`.
5. With `sessions_held = 2`, three sessions in turn and then the first again: the first's next
   call starts a new agent in the store.
6. A two-agent session (a spawn tool call and the child's first message) and one refused call run
   through the binary against a fake upstream on loopback; read back with `sessionspec-records`'
   reader: both agents, the parent link, the tool calls, the gateway facts on each model turn, the
   refused turn marked incomplete, and no record for a count-tokens call sent in the same session.
7. The test decodes every stored command through that reader and searches it for each prompt and
   tool input it sent and finds none; the same search over one command in which the test planted
   a prompt finds it.
8. For each header source the gateway's session hash equals
   `sessionspec_records::wire::session_hash` of the same source and value.
9. The binary tests start the binary on port 0 and read its bound port (story:cli-test-port-race)
   and are one new module of `crates/llm-gateway-cli/tests/main.rs`; each under 5 s.
10. A guide page names the table, its keys, the session bound and its effect, and how
    `sessionspec serve` reads the file.
