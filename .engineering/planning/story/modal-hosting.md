---
format: aep.planning-md/3
id: story:modal-hosting
kind: story
status: draft
title: Modal supplies the hosting contract without simulated capabilities
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 2
---
## Context

Verify the current official deployment/control contract before choosing a Rust integration boundary. Do not invent a general REST deployment API or treat a pre-existing endpoint as provisioning proof. Capture any unsupported control-plane action as a blocker; use existing-endpoint routing independently. No new Python runtime without an explicit language decision.

## Acceptance

A documented Modal control-plane binding passes lifecycle fixtures and separately records a live deployment/readiness/cleanup result for its supported lifecycle.

## Evidence

Operator-approved design, 2026-09-19. The design and catalog domain it was drafted from are llm's, not this repository's: `docs/design.md`, `spec/system.yaml` and `spec/domains/catalog.yaml` in https://github.com/beyond10x/llm at `8d8e752d`. In this repository the hosting contract is `docs/hosting.md` and `spec/domains/hosting.yaml`; Modal has no domain or contract here yet (`crates/llm-modal` exports nothing).

## Verification

Retain commands and exact fixture/contract identities demonstrating the acceptance and its named failure cases. A passing scaffold build is not runtime evidence. No paid call runs in the normal gate.

## Scope

- inferred: `crates/llm-modal` — planned implementation surface.
- inferred: `contracts/modal` — planned implementation surface.

Shared specification and workspace manifests are integration surfaces: coordinate changes through their owning story; do not infer parallel safety from different crate names.

## Moved

Moved from `beyond10x/llm` (`story:modal-hosting`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/modal-hosting/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
