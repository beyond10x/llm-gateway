---
format: aep.planning-md/3
id: story:ess-gateway
kind: story
status: implemented
title: The gateway surface is specified in ESS
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:17:01Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "proposed", to: "active", at: "2026-10-05T10:17:02Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "active", to: "implemented", at: "2026-10-05T10:17:02Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Operator rule, 2026-09-26: every non-tooling beyond10x repository is driven by ESS. `crates/llm-gateway`
has no domain of its own in `spec/domains/` (read from `origin/plan/llm-foundation`, 2026-09-26):
its authenticated single-owner surface and
read-only route inspection, implemented in wave 1 (`story:gateway-auth`), are proved by crate
tests only. This story retrofits it: declarations read from the shipped code and citing it, anything
not readable marked `UNMAPPED:`.

## Acceptance

An `llm.gateway` domain declares the owner authentication and route inspection the
crate ships, and its scenarios pass against the real gateway over a loopback socket.

## Evidence

`crates/llm-gateway`; `docs/gateway.md`; `story:gateway-auth`.

## Verification

Authored `ess-scenario/1` documents under the domain's scenario directory run through
`checks/conformance` against the real crate, three runs with identical counts and zero failed,
error, unsupported or skipped; every guarded behaviour has a falsification record. The whole
repository `conformance -- check` stays green. No paid provider call in the default gate.

## Moved

Moved from `beyond10x/llm` (`story:ess-gateway`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/ess-gateway/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
