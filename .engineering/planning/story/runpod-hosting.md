---
format: aep.planning-md/3
id: story:runpod-hosting
kind: story
status: implemented
title: Runpod provides recoverable vLLM deployments
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:16:58Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "proposed", to: "active", at: "2026-10-05T10:16:58Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "active", to: "implemented", at: "2026-10-05T10:16:59Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Port existing llmgw behavior with attribution. Ordered GPU choices, mounted caches, startup deadlines and active-stream leases remain explicit. Prevent overlap with the legacy llmgw namespace/controller. Live paid verification is separately recorded and never part of the default gate.

## Acceptance

Emulated Runpod tests prove single-flight startup, readiness/crash recovery, ownership-safe adoption and reachable idle/orphan cleanup using declared vLLM settings.

## Evidence

Operator-approved design, 2026-09-19; docs/design.md; spec/system.yaml and spec/domains/catalog.yaml. Existing source references are listed under docs/design.md, Source evidence and draft limits.

## Verification

Retain commands and exact fixture/contract identities demonstrating the acceptance and its named failure cases. A passing scaffold build is not runtime evidence. No paid call runs in the normal gate.

## Scope

- inferred: `crates/llm-runpod` — planned implementation surface.
- inferred: `contracts/runpod` — planned implementation surface.

Shared specification and workspace manifests are integration surfaces: coordinate changes through their owning story; do not infer parallel safety from different crate names.

## Moved

Moved from `beyond10x/llm` (`story:runpod-hosting`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/runpod-hosting/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
