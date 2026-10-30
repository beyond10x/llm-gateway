---
format: aep.planning-md/3
id: review-result:adversary-w06-spec-diff-gate-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: spec-diff-gate (w06)'
relations:
- reviews: story:spec-diff-gate
revision: 1
---
Adversary pass 1 on story:spec-diff-gate (impl/spec-diff-gate, subject f3c076f4...99726a67; cases committed as d94c6456).

```
unit: story:spec-diff-gate, branch impl/spec-diff-gate at d94c6456 (subject f3c076f4...99726a67)
verdict: NEEDS-CHANGE (story acceptance 3/4 text); both red cases are CONFIRMED, severity note
cases: executed 19→24, red 2
origin: introduced 2 / pre-existing 1 / undecided 0
wrote-outside-worktree: 8 files in the unit's scratch adv1/
needs-coordinator: rewrite story acceptance 3 and 4 for ESS 0.77.0
```

Cases added in `checks/conformance/tests/adversary_spec_diff.rs`: `without_ess_on_path_the_refusal_names_ess` (red),
`without_ess_on_path_no_base_copy_is_left_behind` (red),
`an_acknowledgement_committed_with_its_change_is_applied_on_the_pull_request_and_the_push` (green),
`the_compared_base_is_the_fork_point_when_origin_main_has_moved_on` (green),
`the_materialised_base_is_removed_after_a_passing_and_a_failing_run` (green).

Red run: `cargo test -p b10x-llm-gateway-conformance --test adversary_spec_diff`: 3 passed; 2 failed.
Suite run with `--bin b10x-llm-gateway-conformance --test spec_diff --test adversary_spec_diff`: bin 11 passed,
adversary 3 passed 2 failed, spec_diff 8 passed, EXIT=101. Clippy 1.98.0 -D warnings EXIT=0.

Attacked and not broken: injection through `--base` (single argument to `rev-parse --verify`), when the
acknowledgements file applies (local, PR merge ref, push to main, branch that merged main, main deleting the
file), digests independent of the per-process directory, exit codes 0/4/1 relayed, `before` all zeros
unreachable (push trigger is main only), shallow clone (`fetch-depth: 0`), gate.yml permissions and SHA pins,
`ls-tree` path matching, symlinks in `spec/` (none), old spec bases refused with exit 1.

```findings
- file: checks/conformance/src/diff.rs
  line: 132
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: without ess on PATH the gate exits 1 with a bare Debug-printed os error that does not name ess
- file: checks/conformance/src/diff.rs
  line: 119
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: any error between materialise and the cleanup at :135 leaves target/spec-diff/base-<pid>/ behind
- file: story:spec-diff-gate
  category: acceptance
  severity: note
  verdict: NEEDS-CHANGE
  origin: pre-existing
  message: acceptance 3 and 4 expect an added domain to fail as unclassified, but ESS 0.77.0 classifies it as compatible domain/<name>/added and the gate passes it
```
