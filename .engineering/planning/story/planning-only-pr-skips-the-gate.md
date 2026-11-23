---
format: aep.planning-md/3
id: story:planning-only-pr-skips-the-gate
kind: story
status: draft
title: A planning-only pull request passes the required checks without building or testing
revision: 1
---
## Why

Workspace delivery rule 3, "Planning-only pull requests skip the tests" (`~/beyond10x/AGENTS.md`,
Timo's "ROLL the 5", 2026-11-21): a pull request whose changed files are all planning or prose
passes its required checks without building or testing.

This repository does not do that yet. PR #58 (2026-11-23) changed one file,
`.engineering/planning/dependency-blocker/llm-usage-decoders.md`, and its required
`serving-crates` job ran the full gate for 6 min 45 s. `.github/workflows/gate.yml` runs on every
`pull_request` with no changed-path check.

## Acceptance

- A pull request whose changed files are all under `.engineering/` or are `*.md` outside the built
  docs site passes `serving-crates` without building or testing: the job runs, finds only such
  paths, and exits 0 within a minute.
- A pull request that changes any other file runs the full gate as before.
- A change under the built docs site still runs the docs check (`pages.yml`); only the code jobs
  skip.
- The path list lives in one place in the workflow, and a test case covers both sides (a
  planning-only change set skips, a change set with one code file does not), in POSIX sh or Rust,
  no Python.
- The required check names stay the same; the ruleset and branch protection are not changed.

## Reference

llm and inference build the same rule in their waves of 2026-11-23
(`story:gate-test-budget-and-planning-skip` in each); reuse their path list and test once merged.
