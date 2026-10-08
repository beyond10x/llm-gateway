---
format: aep.planning-md/3
id: dependency-blocker:llm-model-factory
kind: dependency-blocker
status: open
title: llm builds no Model from a catalog serving model (0.3.1)
relations:
- blocks: story:loom-qualification
revision: 1
---
## What is missing

beyond10x/llm 0.3.1 has no function that builds the `Model` a catalog serving model declares.
Every caller writes its own `match` on `Protocol` (`crates/llm-routing/src/fallback.rs:22-24`,
`crates/llm-docs/examples/local_endpoint.rs:25-34`).

## Cleared when

An llm release exports that function for the three protocol clients and the credential resolvers.
The change belongs in beyond10x/llm.
