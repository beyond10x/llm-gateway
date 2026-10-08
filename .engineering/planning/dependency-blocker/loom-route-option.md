---
format: aep.planning-md/3
id: dependency-blocker:loom-route-option
kind: dependency-blocker
status: open
title: b10x-loom cannot name an llm catalog route
relations:
- blocks: story:loom-qualification
revision: 1
---
## What is missing

The `b10x-loom` command line builds only `codex_model` for `--model` and `--classifier-model`
(beyond10x/loom `crates/loom-cli/src/main.rs:145,202`). It cannot name an llm catalog or a route.

## Cleared when

A loom release takes an llm catalog file and a route alias for both models. The change belongs in
beyond10x/loom.
