---
format: aep.planning-md/3
id: story:spec-diff-gate
kind: story
status: implemented
title: A breaking or unclassified spec change fails the gate until acknowledged
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
- depends_on: story:ess-0-53
- depends_on: story:gateway-spec-declarations
- depends_on: story:hosting-spec-declarations
scope:
- confidence: cited
  path: .github/workflows/gate.yml
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: README.md
- confidence: cited
  path: Taskfile.yml
- confidence: cited
  path: checks/conformance/src/diff.rs
- confidence: cited
  path: checks/conformance/src/main.rs
- confidence: cited
  path: checks/conformance/tests/spec_diff.rs
- confidence: cited
  path: website/docs/guides/run-the-checks.md
revision: 17
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T18:08:39Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-30T18:08:39Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-30T20:49:00Z", actor: "human:timo", revision: 17, decided_on: {"recorded":{"test_result":1,"review_outcome":6,"verification":1}}}
---
## Outcome

`task check` and CI compare `spec/` with the specification at the merge base, using
`ess verify diff --fail-on breaking-or-unknown`. A breaking or unclassified change fails the gate
unless `contracts/diff-acknowledgements.json` names it, and that document is bound to both
endpoint digests.

## Why

`ess:hardening` technique 7 (`docs/verification/spec-hardening.md`). Today nothing stops a
narrowing, such as a removed refusal or enum variant, from merging as if it were additive. On
2026-10-06 a planted removal of `LeaseExpired` exited 4 as `breaking`. The same command reports
added domains as `unknown` (https://github.com/beyond10x/ess/issues/469). Until that is fixed,
adding a domain needs an acknowledgement.

## Acceptance

1. The gate compares with the specification at the merge base: `origin/main` locally, and the
   pull request's base in CI. Comparing the specification with itself exits 0.
2. With `LeaseExpired` removed in a copy, the gate fails and names
   `type/llm-gateway.hosting.StopReason/variant-removed/LeaseExpired`. With the copy restored,
   it passes.
3. A change ESS 0.77.0 cannot classify (a changed enum attribute value) fails the gate when no
   acknowledgement names `system/llm-gateway/unclassified-changed`; an added domain passes as
   `domain/<name>/added`.
4. The same change passes once `contracts/diff-acknowledgements.json` names that change ID with
   both endpoint digests.
5. An acknowledgement whose digests do not match the compared endpoints is refused.
6. `AGENTS.md` "Gate" lists the step and the acknowledgements file. "Planning and waves" lists the
   acknowledgements file as an integration file.

Acceptance 3 rewritten 2026-10-30 in wave 2026-10-30-w06 from the implementor's measurement on
ESS 0.77.0: an added domain is reported `domain/llm-gateway.probe/added`, compatible, exit 0
(beyond10x/ess issue 469, closed 2026-10-08); a changed `HostingError` message text is reported
`system/llm-gateway/unclassified-changed`, exit 4. The original wording expected the added domain
to fail.

## Who writes the acknowledgements

The file holds only the acknowledgements for the change it ships with, compared against that
change's merge base. A story whose change the gate fails writes its own acknowledgement. Whoever
merges a wave rewrites the file for the merged change, because every acknowledgement is bound to
the digests of one comparison and goes stale when another change merges first.

## Depends on

- `story:ess-0-53`. The `--fail-on` and `--compatibility` flags do not exist in ESS 0.52.0, and
  both stories change `.github/workflows/gate.yml`.
- `story:gateway-spec-declarations` and `story:hosting-spec-declarations`. They delete
  `spec-only` declarations, which this gate classifies as breaking. Landing after them means
  their deletions need no acknowledgement.

## Scope (inferred)

`Taskfile.yml`, `.github/workflows/gate.yml`, a new `contracts/diff-acknowledgements.json`.
