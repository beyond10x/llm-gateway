---
format: aep.planning-md/3
id: story:ess-runpod
kind: story
status: implemented
title: The Runpod adapter is specified in ESS
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:16:59Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "proposed", to: "active", at: "2026-10-05T10:17:00Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "active", to: "implemented", at: "2026-10-05T10:17:00Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Operator rule, 2026-09-26: every non-tooling beyond10x repository is driven by ESS. `crates/llm-runpod`
has no domain of its own in `spec/domains/` (read from `origin/plan/llm-foundation`, 2026-09-26):
the wave 2 adapter proves single-flight
startup, recovery, ownership and cleanup with 52 crate cases and no scenario. This story retrofits it: declarations read from the shipped code and citing it, anything
not readable marked `UNMAPPED:`.

## Acceptance

An `llm.runpod` domain declares the adapter's lifecycle over the `llm.hosting`
contract, and its scenarios pass against the adapter with the in-process emulator.

## Evidence

`crates/llm-runpod`; `docs/verification/runpod.md`; `story:runpod-hosting`.

## Verification

Authored `ess-scenario/1` documents under the domain's scenario directory run through
`checks/conformance` against the real crate, three runs with identical counts and zero failed,
error, unsupported or skipped; every guarded behaviour has a falsification record. The whole
repository `conformance -- check` stays green. No paid provider call in the default gate.

## Moved

Moved from `beyond10x/llm` (`story:ess-runpod`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/ess-runpod/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
