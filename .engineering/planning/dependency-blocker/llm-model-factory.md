---
format: aep.planning-md/3
id: dependency-blocker:llm-model-factory
kind: dependency-blocker
status: cleared
title: llm builds no Model from a catalog serving model (0.3.1)
relations:
- blocks: story:loom-qualification
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T10:03:42Z", actor: "human:timo", revision: 3}
---
## What is missing

beyond10x/llm 0.3.1 has no function that builds the `Model` a catalog serving model declares.
Every caller writes its own `match` on `Protocol` (`crates/llm-routing/src/fallback.rs:22-24`,
`crates/llm-docs/examples/local_endpoint.rs:25-34`).

## Cleared 2026-10-08 by llm 0.4.0

beyond10x/llm 0.4.0 (https://github.com/beyond10x/llm/releases/tag/0.4.0) carries llm's
`story:catalog-model-port`, the catalog-to-`Model` function. `story:loom-qualification` takes llm at
tag `0.4.0` or newer when it adds the dependency; no crate here depends on llm today.

## Cleared when

An llm release exports that function for the three protocol clients and the credential resolvers.
The change belongs in beyond10x/llm.
