---
format: aep.planning-md/3
id: review-result:adversary-w06-credential-buffers-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: credential-buffers-zeroed (w06)'
relations:
- reviews: story:credential-buffers-zeroed
revision: 1
---
Adversary pass 1 on story:credential-buffers-zeroed (impl/credential-buffers-zeroed at a98c451; cases
committed as 9da77d1 in `crates/llm-gateway-cli/tests/adversary_w06_record.rs`).

```
unit: U4 story:credential-buffers-zeroed, branch impl/credential-buffers-zeroed at a98c451 (adversary commit 9da77d1 on top)
verdict: NEEDS-CHANGE
cases: executed 192→194 on a98c451's lanes (+75 in 5 more targets), red 3 (2 added, 1 existing)
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 7 paths, all under the assigned adv1/ scratch
needs-coordinator: the existing red case comes from coordinator commit a98c451; the min(limit) case belongs in server.rs head_tests
```

Red: `adv_w06_the_record_names_the_test_of_every_buffer_the_branch_sized`,
`adv_w06_the_record_lists_as_unchanged_no_site_the_branch_changed` (the verification record still lists
`sha256`, `owner_verifier` and `TokenFile::resolve` as unchanged), and the existing
`adversary_pass_2::the_verification_records_case_count_is_the_count_the_suite_declares` (a98c451's new
`#[test]` in auth.rs makes the six sources declare 48 cases against `docs/verification/gateway.md:9`'s 47).
Mutation probe in scratch: dropping `.min(limit)` at `server.rs:684` survives both `head_tests`; a
3000-byte-bound case read 700 bytes at a time catches it.

Suite: gateway lib 50, spec_declarations 3, dependency_boundary 10, gateway 24, observability 10 ok,
adversary_pass_2 6/7; cli lib 47, adversary_w02 8, bedrock 26, binary 48, credential_set 17, hosted 17 ok,
adversary_w06_record 0/2. EXIT=101.

Not broken: `read_head` loop order, terminator and stray-byte checks, bare-LF, NUL, split CR, Interrupted,
deadline and refusal codes; head allocation never past 8192 under the default bound; trusted reader outcomes
at, over and growing past the bound; key reader trimming and stdin bound; probe request bytes and capacity;
sha256 padding for every length and the Bedrock version digest; the test ledger trips every realistic mutant
of the wrapper; no `unsafe` in the diff.

```findings
[
{"file":"crates/llm-gateway/src/auth.rs","line":485,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a98c451's new #[test] in auth.rs makes the six sources declare 48 cases against docs/verification/gateway.md:9's 47, so adversary_pass_2::the_verification_records_case_count_is_the_count_the_suite_declares is red and task check fails."},
{"file":"docs/verification/2026-10-30-credential-buffers-zeroed.md","line":75,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The record still lists sha256, owner_verifier and TokenFile::resolve as unchanged and leaking and has no table row for them, though a98c451 fixed all three."},
{"file":"CHANGELOG.md","line":12,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The Unreleased line omits the owner-token hashing, owner secret text and Bedrock token that a98c451 fixed."},
{"file":"crates/llm-gateway/src/server.rs","line":684,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Removing .min(limit) from HeadBuffer growth survives both head_tests because their bounds are reached exactly by doubling 1024; a 3000-byte-bound case catches it."}
]
```
