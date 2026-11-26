---
format: aep.planning-md/3
id: review-result:critics-interaction-records
kind: review-result
status: active
title: 'Plan critics on the interaction-records set: approve after three rounds'
relations:
- reviews: story:finished-call-facts
- reviews: story:interaction-record-store
- reviews: epic:interaction-records
revision: 1
---
## Verdict

approve, from all four plan critics (acceptance, design, parallel safety, scope), on
epic:interaction-records, story:finished-call-facts and story:interaction-record-store at revision
b75a9ab5.

## Rounds

1. At 35956006: needs-revision from all four. Acceptance (6): the no-text check grepped
   zstd-compressed blobs and could not fail; "answer time unchanged" had no measure; the outcome
   without the table, the session bound, count-tokens and refused calls had no acceptance; the
   epic had no Done when. Design (5): the port's signature split between the facts and tap
   stories; the dropped-calls series needs the gateway crate's metrics; a third copy of the
   dashboard's facts; no rule for an over-bound call; no test that the two session-hash
   functions agree. Parallel safety (5): the shared port and files of facts and tap; the series
   files; the test files the one-binary move rewrites. Scope (6): the live channels, the
   `BOUNDARY:` comment and the no-key-field choice, the minor version step, count-tokens, the hash
   join, and a session bound the design did not name.
2. At b926e878: story:call-bytes-tap merged into story:finished-call-facts and archived; the store
   and the epic rewritten. Design, parallel safety and scope approve. Acceptance (1): no
   acceptance read the skipped-calls series.
3. At b75a9ab5: the store's acceptance 4 runs one over-bound call and reads the series.
   Acceptance approves.

## Notes the critics left outside their lanes

- story:finished-call-facts covers a wide surface in one story (relay, server, dashboard, usage,
  telemetry); no other item in the set touches those files.
- Neither story carries typed scope; `aep plan artifact waves` lists both as unassessed.
