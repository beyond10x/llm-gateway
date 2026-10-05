---
format: aep.planning-md/3
id: story:gateway-auth
kind: story
status: implemented
title: The gateway admits one authenticated owner
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:17:00Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "proposed", to: "active", at: "2026-10-05T10:17:01Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1,"verification":1}}}
- {from: "active", to: "implemented", at: "2026-10-05T10:17:01Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Context

Compose injected credential sources and verifier for local or remote single-owner deployment; never assume a desktop keychain on servers. Authenticated context is supplied before business decoding. Define health/readiness and graceful shutdown. Multi-tenant accounts/quotas are outside this milestone.

## Acceptance

Unauthenticated requests are refused and an authenticated owner can inspect routes without revealing upstream credentials or triggering provisioning.

## Evidence

Operator-approved design, 2026-09-19; docs/design.md; spec/system.yaml and spec/domains/catalog.yaml. Existing source references are listed under docs/design.md, Source evidence and draft limits.

## Verification

Retain commands and exact fixture/contract identities demonstrating the acceptance and its named failure cases. A passing scaffold build is not runtime evidence. No paid call runs in the normal gate.

## Scope

Derived 2026-09-20 by `story-scoper`. Every line is **cited** (read from the artifact or an opened
file in this tree) or **inferred** (worked out from the four implemented domains, and it could be
wrong). Wave-1 boundary: authentication, health/readiness, graceful shutdown and read-only route
inspection. Protocol translation belongs to `story:gateway-translation` and is not scoped here.

Owned outright — no other wave-1 candidate names these:

- cited: `crates/llm-gateway` — the artifact's own scope entry. The crate exists as a scaffold: `src/lib.rs` is a doc comment ending "Planning scaffold only. This crate exports no runtime API yet.", and `Cargo.toml` has no `[dependencies]` table at all. The verifier, the injected credential-source composition, the health/readiness surface, graceful shutdown, the read-only route inspection handler, the crate manifest and the crate's new `tests/` all land inside this directory.
- inferred: `spec/domains/gateway.yaml` — every implemented domain owns one domain file (`routing.yaml`, `secrets.yaml`, `budget.yaml`, `accounting.yaml`, `inference.yaml`), added in the same commit as its crate. The filename and the `llm.gateway` domain name are guessed; the existence of a new domain file is not, because `epic:gateway` requires this story's contracts in `story:foundation-qualified`.
- inferred: `contracts/gateway/scenarios` — authored scenarios live one directory per domain (`contracts/{budget,inference,pricing,routing,secrets}/scenarios`, 22–49 files each). `story:gateway-translation` already carries `contracts/gateway` in its scope, so the directory name is the project's own expectation; that story is not in wave 1.
- inferred: `checks/conformance/src/gateway.rs` — a new adapter module beside `budgets.rs`, `secrets.rs`, `inference.rs`, `pricing.rs`, one per domain.
- inferred: `docs/gateway.md` — the contract narrative, following `docs/budgets.md`, `docs/local-secrets.md`, `docs/pricing.md`. Already recorded in this story's store scope.
- inferred: `docs/verification/gateway.md` — the verification record, following `docs/verification/{budgets,local-secrets,observations,pricing}.md`.
- inferred: `docs/verification/gateway-report.json` — the retained ESS report, following `budget-report.json`, `local-secrets-report.json`, `observations-report.json`, `pricing-report.json`, `routing-report.json`.
- inferred: `docs/verification/gateway-falsification.json` — the retained mutation record, following the four existing `*-falsification.json` files.

Shared with other wave-1 candidates — these are the lines this section exists for:

- cited: `checks/conformance/src/target.rs` — read. One `CatalogTarget` dispatches every domain from a single `match request.command.to_string().as_str()` (lines 146–182: `llm.inference.InspectResult`, `llm.routing.Evaluate`, `llm.secrets.ProbeFile`, `llm.secrets.ProbeKeychain`, `llm.accounting.Quote`, `llm.budget.Exercise`) plus a view whitelist at lines 217–221. Every domain-adding story edits both blocks. All four implemented-domain commits touched this file.
- cited: `checks/conformance/src/main.rs` — read. The module registry is six literal lines (`mod budgets; mod gate; mod inference; mod pricing; mod secrets; mod target;`). Each new adapter adds one, at the top of the file, where every other story adds one too.
- cited: `checks/conformance/Cargo.toml` — read. Each domain adds its crate as a path dependency (`llm-cost`, `llm-credentials`, `llm-routing` are all listed individually); this story adds `llm-gateway`.
- cited: `spec/system.yaml` — named in the story's Evidence section. Read: a single `domains:` list of seven entries, no gateway among them. Every one of the four implementation commits edited this list; it is the single most contended file of the wave.
- cited: `docs/design.md` — named in the story's Evidence section. The "Gateway and hosting" section (line 51) states the acceptance almost verbatim ("Listing or explaining routes must not resolve secrets or provision resources"), and the "Source evidence and draft limits" paragraph carries the draft limits this story retires. Three of four implementation commits edited it.
- cited: `docs/contract-v1.md` — exists; edited by all four implemented-domain commits as the published cross-domain contract.
- inferred: `README.md` — line 13 currently asserts "There is no usable gateway, provider client, release or deployment yet", which this story falsifies in part. Three of four implementation commits edited it.
- inferred: `.github/workflows/gate.yml` — three of four implementation commits added per-crate build/test lines here as each crate gained features. Needed only if `llm-gateway` ships feature-gated code; skippable otherwise.
- inferred: `spec/domains/catalog.yaml` — **avoidable, and should be avoided.** Route inspection reuses the existing `llm.catalog` entities unchanged, so this story should need no edit; it is listed only because `story:hosting-contract` claims this exact file and a stray gateway record added here is a silent conflict. Keep any new gateway record in `spec/domains/gateway.yaml`.

Not in scope, checked and rejected: `crates/llm-routing` and `crates/llm-credentials` are consumed, not modified — inspection reads an existing `Catalog` and the credential *sources* are injected, while the inbound owner *verifier* is new code in `llm-gateway`. `checks/conformance/src/gate.rs` is domain-agnostic: it walks `spec/`, `contracts/` and hashes `crates`/`checks`, so a new domain needs no edit there.

## Moved

Moved from `beyond10x/llm` (`story:gateway-auth`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/gateway-auth/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
