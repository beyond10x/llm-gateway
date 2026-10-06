---
format: aep.planning-md/3
id: story:spec-diff-gate
kind: story
status: draft
title: A breaking or unclassified spec change fails the gate until acknowledged
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
- depends_on: story:ess-0-53
- depends_on: story:gateway-spec-declarations
- depends_on: story:hosting-spec-declarations
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: contracts/diff-acknowledgements.json
revision: 7
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
3. A change that adds a domain fails the gate when no acknowledgement names
   `system/llm-gateway/unclassified-changed`.
4. The same change passes once `contracts/diff-acknowledgements.json` names that change ID with
   both endpoint digests.
5. An acknowledgement whose digests do not match the compared endpoints is refused.
6. `AGENTS.md` "Gate" lists the step and the acknowledgements file. "Planning and waves" lists the
   acknowledgements file as an integration file.

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
