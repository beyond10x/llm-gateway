---
format: aep.planning-md/3
id: story:hosting-contract
kind: story
status: implemented
title: Hosting has explicit owned-resource lifecycle semantics
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:16:57Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "proposed", to: "active", at: "2026-10-05T10:16:57Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "active", to: "implemented", at: "2026-10-05T10:16:57Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Resolve UNMAPPED hosting transitions before implementation. Separate DeploymentSpec from observed ProvisionedDeployment. Document mutation ambiguity, idempotency, cancellation, ownership fencing, max resources/time and cost-policy integration. Listing/validation must never allocate compute.

## Acceptance

A fake hosting provider demonstrates provisioning, readiness, concurrent acquisition, active leases, restart reconciliation and cleanup against a validated ownership state machine.

## Evidence

Operator-approved design, 2026-09-19; docs/design.md; spec/system.yaml and spec/domains/catalog.yaml. Existing source references are listed under docs/design.md, Source evidence and draft limits.

## Verification

Retain commands and exact fixture/contract identities demonstrating the acceptance and its named failure cases. A passing scaffold build is not runtime evidence. No paid call runs in the normal gate.

## Scope

Derived 2026-09-20 by `story-scoper`. Every bullet is **cited** (read from the artifact or from a file in this tree) or **inferred** (worked out from the four completed domain landings and not read anywhere).

- cited: `crates/llm-provision` — the story's own scope entry, and the crate is a two-file scaffold (`Cargo.toml`, `src/lib.rs`, "Planning scaffold only. This crate exports no runtime API yet.") whose description is already "Hosting lifecycle ports and resource ownership contracts." The ownership state machine, the fake hosting provider and its tests all land here.
- cited: `spec/domains/hosting.yaml` — the story's own scope entry; no such file exists yet. The acceptance needs a validated ownership state machine, which in this repo means a domain with real lifecycle states, as `spec/domains/budget.yaml` does.
- cited: `spec/domains/catalog.yaml` — named in the story's Evidence and in its scope. `llm.catalog.DeploymentSpec` (line 196) and `llm.catalog.ProvisionedDeployment` (line 209) live there today, both with the placeholder lifecycle `initial: Declared / states: [Declared]`. "Separate DeploymentSpec from observed ProvisionedDeployment" is an edit to those two entities.
- cited: `spec/system.yaml` — named in the story's Evidence. Its `domains:` list has seven entries and no `llm.hosting`; a new domain file is inert until registered here, exactly as `183a8a1` registered `llm.budget`.
- cited: `docs/design.md` — named in the story's Evidence, and it carries the defect the Context is written against: "UNMAPPED: provider-specific hosting lifecycle transitions await their design story. They must be modeled before those controllers are implemented, including consumption of budget stop obligations." Resolving UNMAPPED means rewriting that paragraph and the "Gateway and hosting" section above it.
- cited: `contracts/hosting/scenarios` — the story's own scope entry; a new authored-scenario directory alongside `contracts/{budget,inference,pricing,routing,secrets}/scenarios`.
- cited: `checks/conformance/src/hosting.rs` — the story's own scope entry; a new adapter module alongside `budgets.rs`, `inference.rs`, `pricing.rs`, `secrets.rs`.
- inferred: `checks/conformance/src/main.rs` — a new adapter module is dead code until declared; the file opens with six `mod` lines and every prior domain added one.
- inferred: `checks/conformance/src/target.rs` — `CatalogTarget` is the single dispatcher; lines 148–178 are one arm per domain (`crate::inference::observe`, `crate::secrets::file`, `crate::pricing::observe`, `crate::budgets::observe`). Hosting needs an arm there.
- inferred: `checks/conformance/Cargo.toml` — the conformance binary reaches each domain crate by an explicit path dependency (`llm-cost`, `llm-routing`, `llm-credentials`); it has none on `b10x-llm-provision`.
- inferred: `docs/hosting.md` — the per-domain design document, the shape of `docs/budgets.md` and `docs/pricing.md`. The Context's list — mutation ambiguity, idempotency, cancellation, ownership fencing, max resources/time, cost-policy integration — is a document, not a code comment.
- inferred: `docs/verification/hosting.md` — the retained verification record the Verification section demands; every landed domain has one.
- inferred: `docs/verification/hosting-report.json` — the `report/2` evidence artefact, as `budget-report.json` and `routing-report.json`.
- inferred: `docs/verification/hosting-falsification.json` — the intentional-defect record, as `budget-falsification.json`.
- inferred: `docs/contract-v1.md` — the published v1 contract names each domain as it lands (line 6 points at `spec/domains/catalog.yaml`, line 71 describes `llm.budget/1`); a hosting contract story plausibly adds its paragraph. This is the weakest entry in the set.

Confidence: **high** for the primary surface and the specification files — the artifact names them and `spec/domains/catalog.yaml:196-218` and `docs/design.md:82-83` are the exact sites. **Medium** for the conformance wiring and the document set, which are read off the established pattern (`183a8a1`, `15a6167`, `d36e4e1`, `21be009`) rather than out of this story.

The dominant surface is `crates/llm-provision`, which nothing else in the candidate set touches. The risk is not there: it is the four files every domain-adding story must edit in common — `spec/system.yaml`, `checks/conformance/src/main.rs`, `checks/conformance/src/target.rs`, `checks/conformance/Cargo.toml` — plus `docs/design.md`, whose "Gateway and hosting" section is shared with the gateway work. Shared specification and workspace manifests are integration surfaces: coordinate changes through their owning story; do not infer parallel safety from different crate names.

## Moved

Moved from `beyond10x/llm` (`story:hosting-contract`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/hosting-contract/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
