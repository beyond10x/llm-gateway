---
format: aep.planning-md/3
id: upstream-blocker:llm-runpod-provider
kind: upstream-blocker
status: cleared
title: No llm release describes Runpod as a provider (design choice D1 = A)
relations:
- blocks: story:runpod-production-transport
- blocks: story:provider-key-files
revision: 6
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T13:34:37Z", actor: "human:timo", revision: 4}
---
## What is missing

Design choice D1 = A (`decision-blocker:runpod-control-plane`) describes Runpod as a provider in
beyond10x/llm `crates/llm-providers`. No llm release carries that description yet.

## Cleared when

An llm release carries the Runpod provider description. Record its tag here and rewrite
`story:runpod-production-transport` and `story:provider-key-files` for A before either starts.

## Cleared 2026-10-08 by llm 0.5.0

beyond10x/llm 0.5.0 (https://github.com/beyond10x/llm/releases/tag/0.5.0) carries
`llm_providers::ProviderDescription` (format `llm.provider-description/1`) and
`descriptions::runpod()`, file `crates/llm-providers/descriptions/runpod.toml`: inference
`https://{instance}-8000.proxy.runpod.net/v1/` with `bearer` auth, and the control plane
`https://rest.runpod.io/v1/openapi.json` with operations CreatePod, ListPods, GetPod and DeletePod.
`story:provider-key-files` was rewritten for D1 = A; `story:runpod-provider-description` takes the
pod address from it. `story:runpod-production-transport` stays blocked by
`upstream-blocker:connectors-runpod-bundle`.
