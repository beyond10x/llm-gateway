---
format: aep.planning-md/3
id: story:orphan-termination-obligations
kind: story
status: draft
title: Orphan terminations record and discharge a stop obligation
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
- depends_on: story:hosting-spec-declarations
- depends_on: story:runpod-production-transport
scope:
- confidence: inferred
  path: checks/conformance/src/hosting.rs
- confidence: inferred
  path: checks/conformance/src/runpod.rs
- confidence: cited
  path: crates/llm-provision/src/controller.rs
- confidence: inferred
  path: crates/llm-provision/src/lib.rs
- confidence: inferred
  path: crates/llm-provision/src/machine.rs
- confidence: inferred
  path: crates/llm-provision/tests/hosting.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: inferred
  path: crates/llm-runpod/tests/runpod.rs
- confidence: cited
  path: docs/hosting.md
- confidence: inferred
  path: spec/domains/hosting.yaml
- confidence: cited
  path: spec/domains/runpod.yaml
revision: 14
---
## Acceptance

Filed from wave 3 (2026-09-27) to own a `DEFERRED:` note in `spec/domains/runpod.yaml`. The
acceptance is written when the story is scoped.

## Moved

Moved from `beyond10x/llm` (`story:orphan-termination-obligations`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/orphan-termination-obligations/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-runpod/src/pool.rs`: `stop_inherited` (pool.rs:614-640) and `sweep_orphans` (pool.rs:743-787), the two terminations `docs/hosting.md:316-320` says bypass the controller — cited
- **Files:** `crates/llm-provision/src/controller.rs`: `Controller`, which the `DEFERRED:` note says the orphan and inherited stops bypass, and where obligations open and close (controller.rs:899, :913) — cited
- **Files:** `spec/domains/runpod.yaml:15-19`: the `DEFERRED:` note this story owns and removes — cited
- **Also likely:** `crates/llm-provision/src/machine.rs`: a `StopReason` variant or a record kind for a resource with no deployment — inferred
- **Also likely:** `crates/llm-provision/src/lib.rs`: re-export of any new public type — inferred
- **Also likely:** `spec/domains/hosting.yaml`: the spec-first declaration of the new command, `StopReason` variant or `Totals` field — inferred
- **Also likely:** `checks/conformance/src/hosting.rs`: a step for any new `HostingCommand` (hosting.rs:437-505); `deployment_fact` reads `record.requested` and `record.authorization` (hosting.rs:668-674) — inferred
- **Also likely:** `checks/conformance/src/runpod.rs`: `pool_facts` (runpod.rs:579-620), if a new totals field is observed — inferred
- **Also likely:** `crates/llm-provision/tests/hosting.rs`: controller tests for the new record path — inferred
- **Also likely:** `crates/llm-runpod/tests/runpod.rs`: next to the orphan-sweep tests at :638 and :704, which the capability matrix row L14 cites — inferred
- **Also likely:** `contracts/ess-inputs.yaml`, `contracts/suite.json`, `contracts/baseline.json`: new scenarios, the regenerated suite and the higher floor — inferred
- **Also likely:** `contracts/schema/schema/types/llm-gateway.hosting.StopReason.schema.json`, `contracts/schema/schema/types/llm-gateway.hosting.Totals.schema.json`: regenerated if those types change — inferred
- **Also likely:** `AGENTS.md:37`: the "at least 179" scenario floor moves with `baseline.json` — inferred
- **Documents:** `docs/hosting.md:316-324`, the "Open: orphan and inherited terminations bypass the controller" paragraph — cited
- **Documents:** `docs/llmgw-capability-matrix.md:154`, row L14, which cites `docs/hosting.md:316-324` — cited
- **Symbols:** `RunpodProvider::stop`, `StopReason::OwnershipLost`, `HostingTotals::stop_required`, `DeploymentRecord` — cited
- **Confidence:** medium — the bypass sites and the owning crate are cited; the shape of the fix (new command, new record kind or new reason) is not, because the acceptance is unwritten — inferred
- **Would collide with:** any unit on the Runpod pool's cleanup or restore path (`pool.rs` `reap`/`restore`/`sweep_orphans`), on the hosting controller's command set or record shape (`controller.rs`, `machine.rs`, `hosting.yaml`), on the contracts manifest and generated suite (every unit that adds a scenario, and any ESS upgrade), on the `DEFERRED:` header block of `spec/domains/runpod.yaml`, on the Runpod section of `docs/hosting.md`, or on capability-matrix rows L13-L15 — inferred
- **Safety fact:** only two calls skip the controller: `self.provider.stop` at pool.rs:638 (`stop_inherited`) and pool.rs:778-780 (`sweep_orphans`). Every other stop goes through `HostingCommand::Stop` to controller.rs:638. No binary runs the pool today: `crates/llm-gateway-cli/src/config.rs:11` imports `llm_runpod` for config types only. Step 2, by grepping for `provider.stop` and `llm_runpod`; unproven — cited

Not established: no acceptance exists in either store; an orphan has no `requested` spec or `authorization` to record against (`controller.rs:21-40`), so a new record type or optional fields is a design choice; the budget-ledger receipt needs llm's `llm-cost` by tag or a separate story.

## Depends on

- `story:hosting-spec-declarations` and `story:runpod-production-transport`. All three stories
  change the `DEFERRED:` header of `spec/domains/runpod.yaml`, the Runpod section of
  `docs/hosting.md` and `checks/conformance/src/runpod.rs`, so this one lands after both.

This story has no acceptance yet, and no wave takes it until one is written (replan of
2026-10-06).
