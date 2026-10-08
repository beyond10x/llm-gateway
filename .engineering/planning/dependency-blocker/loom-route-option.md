---
format: aep.planning-md/3
id: dependency-blocker:loom-route-option
kind: dependency-blocker
status: cleared
title: b10x-loom cannot name an llm catalog route
relations:
- blocks: story:loom-qualification
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T10:25:41Z", actor: "human:timo", revision: 4}
---
## What is missing

The `b10x-loom` command line builds only `codex_model` for `--model` and `--classifier-model`
(beyond10x/loom `crates/loom-cli/src/main.rs:145,202`). It cannot name an llm catalog or a route.

## Cleared 2026-10-08 by loom 0.7.0

beyond10x/loom 0.7.0 (https://github.com/beyond10x/loom/releases/tag/0.7.0) carries the catalog
route (https://github.com/beyond10x/loom/pull/36). `story:loom-qualification` runs `b10x-loom` at
0.7.0 or newer; no crate here depends on loom, and design § 3 says the gateway needs nothing for
the route.

## Cleared when

A loom release takes an llm catalog file and a route alias for both models.
