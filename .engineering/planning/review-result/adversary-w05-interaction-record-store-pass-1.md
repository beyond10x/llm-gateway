---
format: aep.planning-md/3
id: review-result:adversary-w05-interaction-record-store-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on interaction-record-store: needs change'
relations:
- reviews: story:interaction-record-store
revision: 1
---
## Verdict

NEEDS-CHANGE on unit U3 of wave 2026-11-26-w05 (story:interaction-record-store), reviewed at wave
commit 9c0c6607. Red cases on adv3/2026-11-26-w05 at b57d0d58: 5 of 5 fail.

## Findings

1. `crates/llm-gateway-cli/src/records.rs:65`: a start on a store of another schema version empties
   it (sessionspec's `Store::open` drops its tables, `crates/sessionspec-records/src/store.rs:122-130`
   at 39c551d6), where the guide promises a refusal; the wire records are the only copy. Introduced.
   Blocker.
2. `records.rs:167`: a batch the writer fails to store is counted only in sessionspec's
   `Stats::failed`, which the binary never reads; once the file is removed every later call fails
   unseen until restart. Introduced.
3. `records.rs:164-170`: a call refused `body-not-json`, or any request the lift cannot read, yields
   an empty batch that `append` reports as queued (sessionspec `append.rs:112-114`): neither stored
   as a refused turn nor counted. Introduced.
4. `crates/llm-gateway/src/metrics.rs:205` and `website/docs/guides/scrape-the-counters.md:49`: the
   skipped series' help says "over the bound" only; an answer that did not decode is counted too.
   Introduced. Note.
5. `records.rs:68`: calls queued at SIGTERM are lost with exit 0 and the usual stop line,
   uncounted and unpublished. Introduced.
6. `website/docs/guides/record-agent-sessions.md:62`: a forgotten session's next agent reuses its
   agent id, so the view shows one agent, not the same agent twice; the unit's test counts
   `StartAgent` commands, not agents. Introduced. Note.
7. `records.rs:148`: per-session lift state grows for the session's life, and each finishing call
   clones its byte copy on the connection thread; the guide names no memory bound. Introduced.
   Note, from reading the code.
8. `crates/llm-gateway/tests/translation_port.rs:525`: fails 1 in 100 runs ("4096 writes, none
   failed"): the whole answer can fit in loopback socket buffers before the client's close. Not
   caused by U3; whether the wave caused it is undecided.

## Held

The lift adds 60 to 100 ms per 8 MiB request in a debug build after the answer is framed, and a
small call of the same session was not held behind a big one. The binary stops within 4 ms after
SIGTERM and the store passes `PRAGMA integrity_check`. No route was found by which message text
reaches the store. Dropped and skipped are exclusive per call.
