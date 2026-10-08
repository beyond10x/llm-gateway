---
format: aep.planning-md/3
id: dependency-blocker:loom-route-option
kind: dependency-blocker
status: open
title: b10x-loom cannot name an llm catalog route
relations:
- blocks: story:loom-qualification
revision: 2
---
## What is missing

The `b10x-loom` command line builds only `codex_model` for `--model` and `--classifier-model`
(beyond10x/loom `crates/loom-cli/src/main.rs:145,202`). It cannot name an llm catalog or a route.

## Upstream state, 2026-10-08

loom's catalog-route story is in https://github.com/beyond10x/loom/pull/36, not yet released; loom
0.6.0 is the newest release. `story:loom-qualification` starts when loom 0.7.0 (or the first
release carrying that pull request) is published.

## Cleared when

A loom release takes an llm catalog file and a route alias for both models. The change belongs in
beyond10x/loom.
