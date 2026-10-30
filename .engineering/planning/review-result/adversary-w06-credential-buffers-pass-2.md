---
format: aep.planning-md/3
id: review-result:adversary-w06-credential-buffers-pass-2
kind: review-result
status: active
title: 'Adversary pass 2: credential-buffers-zeroed (w06)'
relations:
- reviews: story:credential-buffers-zeroed
revision: 1
---
Adversary pass 2 (final) on story:credential-buffers-zeroed, attacking 46e8090 and ebbd419; case committed as
81efc97 in `crates/llm-gateway/tests/adversary_w06_pass_2.rs`.

```
unit: U4 story:credential-buffers-zeroed, impl/credential-buffers-zeroed at ebbd419 (adversary commit 81efc97)
verdict: CONFIRMED
cases: executed 222→223, red 1
origin: introduced 0 / pre-existing 1 / undecided 0
wrote-outside-worktree: 6 paths under the assigned adv2/ scratch
needs-coordinator: the story's typed scope does not list the files the unit changed
```

Red: `adv_w06_the_records_count_with_server_rs_included_is_what_those_files_declare`.
`docs/verification/gateway.md:10` says 52 cases once `src/server.rs` and `tests/adversary_pass_2.rs` are
included; the eight files declare 61 (57 at base f3c076f4). Suite: gateway lib 51, adversary_pass_2 7,
gateway 24, relayed_session 5, security_w06_head_buffer 3, wire_relay 78, adversary_w06_pass_2 0/1; runpod
lib 6, security_w06_probe_buffer 2, transport 41; cli adversary_w06_record 2, security_w06_buffers 3.

Not broken: the relay receives the same bytes as before; every non-relay path overwrites the bytes past the
head, including a panic unwinding through `serve`; each likely mutant of `Leftover` and the ledger trips a
test; the Runpod probe request is byte-identical for every key case and reserved at exactly its length; no
new allocation holds a credential; the record's tests and residuals match the code. A probe of the relay's
`linger` stack array found no credential in memory and was not kept.

Coordinator: the count in `docs/verification/gateway.md` and the record's ledger list are corrected on the
integration branch; the story's Scope is rewritten at wave close from the files the unit changed.

```findings
[
{"file":"docs/verification/gateway.md","line":10,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"The record says 52 cases once src/server.rs and tests/adversary_pass_2.rs are included, but those eight files declare 61 (57 at base f3c076f4), the unit's four head_tests adding to the drift."},
{"file":"docs/verification/2026-10-30-credential-buffers-zeroed.md","line":42,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The ledger section's list of what the head cases cover omits the new pipelined case, which table row 20 names."}
]
```
