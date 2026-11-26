---
format: aep.planning-md/3
id: review-result:adversary-w05-interaction-record-store-pass-2
kind: review-result
status: active
title: 'Adversary pass 2 on interaction-record-store: needs change'
relations:
- reviews: story:interaction-record-store
revision: 1
---
## Verdict

NEEDS-CHANGE on correction 1 of unit U3 of wave 2026-11-26-w05 (story:interaction-record-store),
reviewed at wave commit 96c202e3. Red cases on adv4/2026-11-26-w05 at e11fcaf4: 3 of 3 fail. Pass
1's 5 cases pass. CLI suite with the cases: 642 run, 639 passed, 3 failed, 1 skipped.

## Findings

1. `crates/llm-gateway-cli/src/records.rs:141`: the schema check parses the row as an integer while
   sessionspec's open compares the string, so a row of `04` passes the check and the store is
   emptied. Introduced. Nothing found writes such a row.
2. `records.rs:139`: every error reading the schema row, a busy store included, is reported as "no
   session store", which invites deleting a valid store. Introduced. Only a connection in exclusive
   locking mode reaches it.
3. `records.rs:163`, `relaying.rs:882`: a call that fails to store during the stop's wait, or in
   the last 100 ms before any stop, gets no warning and is not on the stop's line: the failure
   thread ends without a last read and the line counts only queued calls. Introduced. Needs change.
4. `records.rs:166`: one warning line per failed call, unbounded; a removed store under load writes
   one line per call. Introduced, as the decision said.
5. `records.rs:141`: a file with a `meta` table and no schema row, which a first start killed
   between sessionspec's setup statements leaves, is refused on every later start. Introduced.
6. `crates/llm-gateway/tests/translation_port.rs:538`: the test asserts that the host's socket
   buffer maxima sum under 64 MiB and fails on a host tuned above that. Introduced.

## Held

A zero-byte file starts a new store; a symlink to another version is refused; a non-SQLite file and
another program's database are refused and left unchanged; no refusal names the path; a call is
never counted twice; the failure thread does not hold the process; the 2 s drain bound holds with
the writer stuck; the stop line comes last; SIGINT and SIGTERM take one path. The translation test
runs 9 to 24 ms.
