---
format: aep.planning-md/3
id: story:ess-latest
kind: story
status: implemented
title: llm-gateway runs on ESS 0.52.0
relations:
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: .github/workflows
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: checks/conformance
- confidence: inferred
  path: contracts
- confidence: inferred
  path: docs
- confidence: inferred
  path: spec
- confidence: inferred
  path: website
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T11:36:46Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-05T11:36:46Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-05T11:51:08Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Outcome

The repository runs on the newest ESS release, 0.52.0: the `ess` CLI its CI installs and its
`ess-conformance` / `ess-primitives` dependencies name tag `0.52.0` instead of rev `be44a33`
(0.36.0), the suite and schemas are regenerated with it, and `task check` passes.

## Why

Operator rule, 2026-10-05: "ALWAYS upgrade to most recent ESS" (`~/beyond10x/AGENTS.md`, "ESS
version").

## Acceptance

- No `be44a33` remains in `Cargo.toml`, `Cargo.lock` or `.github/workflows/`; both crates and the
  CI install name `0.52.0`.
- `ess specify validate`, the conformance check and `task check` exit 0 with ESS 0.52.0; scenario
  totals are unchanged or each difference is explained.

## ESS first

None: a toolchain move; every refusal or regenerated difference is reported, not hidden.
