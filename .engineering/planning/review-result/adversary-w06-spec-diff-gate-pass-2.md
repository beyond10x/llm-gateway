---
format: aep.planning-md/3
id: review-result:adversary-w06-spec-diff-gate-pass-2
kind: review-result
status: active
title: 'Adversary pass 2: spec-diff-gate (w06)'
relations:
- reviews: story:spec-diff-gate
revision: 1
---
Adversary pass 2 on story:spec-diff-gate (impl/spec-diff-gate at 9f5cab7c; cases committed as 85aa5e3d in
`checks/conformance/tests/adversary_spec_diff_2.rs`).

```
unit: story:spec-diff-gate (U1); findings are for commit 9f5cab7c, cases committed on top as 85aa5e3d
verdict: NEEDS-CHANGE (3 red cases, all notes, none a blocker)
cases: executed 13→16, red 3
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 path (adv2 commit message, kept)
needs-coordinator: none
```

Red cases: `an_ess_on_path_that_cannot_be_executed_is_not_reported_as_missing_from_path`,
`a_base_copy_left_by_a_killed_run_is_removed_by_a_later_run`,
`a_base_with_no_common_history_is_refused_with_a_reason`. Suite: spec_diff 8 passed, adversary_spec_diff 5
passed, adversary_spec_diff_2 0 passed 3 failed (exit 101).

Not broken: 8 concurrent runs in one target; a stale same-pid directory; every subprocess through `spawn`;
panic unwinding runs the guard; acknowledgement digest independent of the base path; push trigger main-only.

```findings
[
  {
    "file": "checks/conformance/src/diff.rs",
    "line": 26,
    "category": "boundary",
    "severity": "note",
    "verdict": "NEEDS-CHANGE",
    "origin": "introduced",
    "message": "spawn appends \"is it on PATH?\" to a Permission denied error for an ess that is on PATH but not executable"
  },
  {
    "file": "checks/conformance/src/diff.rs",
    "line": 139,
    "category": "concurrency",
    "severity": "note",
    "verdict": "CONFIRMED",
    "origin": "introduced",
    "message": "a base-<pid> copy left by a killed run is never removed by any later run, because each run removes only its own pid"
  },
  {
    "file": "checks/conformance/src/diff.rs",
    "line": 45,
    "category": "boundary",
    "severity": "note",
    "verdict": "CONFIRMED",
    "origin": "introduced",
    "message": "a base with no common history is refused as \"git merge-base <sha> HEAD refused: \" with no reason, because git prints nothing to stderr"
  }
]
```

Findings block re-sent by the adversary as JSON after the store refused its YAML form.
Ledger by hand: pass 1 had 3 (2 introduced fixed in 9f5cab7c, 1 acceptance text rewritten); pass 2 has 3
new, 0 carried, 2 resolved.
