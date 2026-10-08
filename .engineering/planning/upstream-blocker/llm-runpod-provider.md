---
format: aep.planning-md/3
id: upstream-blocker:llm-runpod-provider
kind: upstream-blocker
status: open
title: No llm release describes Runpod as a provider (design choice D1 = A)
relations:
- blocks: story:runpod-production-transport
- blocks: story:provider-key-files
revision: 2
---
## What is missing

Design choice D1 = A (`decision-blocker:runpod-control-plane`) describes Runpod as a provider in
beyond10x/llm `crates/llm-providers`. No llm release carries that description yet.

## Cleared when

An llm release carries the Runpod provider description. Record its tag here and rewrite
`story:runpod-production-transport` and `story:provider-key-files` for A before either starts.
