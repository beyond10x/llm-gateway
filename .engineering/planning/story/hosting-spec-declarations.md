---
format: aep.planning-md/3
id: story:hosting-spec-declarations
kind: story
status: draft
title: hosting.yaml and runpod.yaml declare every rule docs/hosting.md promises
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
- depends_on: story:ess-0-53
- depends_on: story:runpod-production-transport
scope:
- confidence: inferred
  path: checks/conformance/src/hosting.rs
- confidence: inferred
  path: crates/llm-provision/tests/hosting.rs
- confidence: inferred
  path: docs/hosting.md
- confidence: inferred
  path: docs/verification/hosting-spec-declarations.md
- confidence: cited
  path: spec/domains/hosting.yaml
- confidence: cited
  path: spec/domains/runpod.yaml
revision: 10
---
## Outcome

`spec/domains/hosting.yaml` and `spec/domains/runpod.yaml` declare every behaviour that
`docs/hosting.md` promises. That includes the closed vocabularies the code already publishes but
the specification does not name: `HostingError`, the stop-evidence kinds and `Completeness`.

## Why

The 2026-10-06 design review (`docs/verification/spec-hardening.md`) found 35 rules missing, 4
spec-only declarations and 3 unclear statements for hosting. Examples: the refusals `wrong-phase`,
`foreign-resource` and `idempotency-unsupported`; one lease per deployment; the fence before any
provider call; the reason-slot precedence; the `max_active` and lifetime ceilings; and the Runpod
safety rules on `llmgw-` names, owner tags, crash windows and idle reaping. A hand mutant that
deleted `Requested -> Disowned` was caught only by the scenario that reads the published table.

## Acceptance

The findings are the rows of `docs/verification/spec-hardening-hosting-review.md` whose status is
`open`: rows 5-39 are `missing`, rows 40-42 are `unclear`, and rows 43-46 are `spec-only`.

1. For each of rows 5-39, the specification declares the rule. It cites an existing scenario in
   `contracts/hosting/scenarios/` or `contracts/runpod/scenarios/`, or a named crate test that
   pins the rule, or this story adds one. A rule ESS cannot hold is an `ESS-LIMIT:` note naming
   the test.
2. `HostingError`, the evidence kinds and `Completeness` are enum types in `hosting.yaml`, with the
   variants the code publishes. A crate test compares each one with its Rust enum, as the
   existing exhaustive-match tests do.
3. Rows 40-42 are answered from the code at the story's base commit. Each answer is a `DECIDED`
   comment beside the declaration it concerns, and a sentence in `docs/hosting.md`:
   - row 40, the crash-restart limit, beside `CrashLoop` in `spec/domains/runpod.yaml`;
   - row 41, the state of an exited pod, beside `ProviderState` in `spec/domains/hosting.yaml`;
   - row 42, whether `pod-exited` and `stopped-serving` reach a caller, beside `PoolRefusal` in
     `spec/domains/runpod.yaml`.
4. Each of rows 43-46 is resolved one of two ways: the declaration is deleted, or a line in
   `docs/hosting.md` justifies it.
5. A new hosting scenario drives a record from `Requested` to `Disowned` through the controller's
   public calls. With `(Phase::Requested, Phase::Disowned)` deleted from `TRANSITIONS`, it fails
   alongside the table scenario.
6. The review file's status column names, for each row, the declaration, test or note that closes
   it.
7. The design-review brief is re-run against the two domains. It returns no `missing` or
   `contradicts` finding that is neither declared nor an `ESS-LIMIT:`. The run is recorded in a
   new `docs/verification/` record.

## Scope (inferred)

`spec/domains/hosting.yaml`, `spec/domains/runpod.yaml`, `docs/hosting.md`, new scenarios under
`contracts/hosting/scenarios/` and `contracts/runpod/scenarios/`,
`checks/conformance/src/hosting.rs`, `crates/llm-provision/tests/hosting.rs`, `docs/verification/`.

## Order changed 2026-10-08

Depends on `story:runpod-production-transport`: both change `spec/domains/runpod.yaml` and
`docs/hosting.md`, and this story now lands after the transport. Rows of
`docs/verification/spec-hardening-hosting-review.md` the transport closes are marked closed by it.
