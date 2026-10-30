---
format: aep.planning-md/3
id: approval-record:wave-2026-10-30-w01-release
kind: approval-record
status: draft
title: Wave 2026-10-30-w01 PR, merge and release 0.17.0 approved by the orchestrator
relations:
- decides: story:credential-namespace-gateway
revision: 1
---
## Stop

Wave 2026-10-30-w01 past its integration branch: one pull request through the bot App, merged to
`main` when green, then release 0.17.0 through `AGENTS.md` § Releases. Also: the store-only move of
`story:credential-namespace-gateway` to implemented rides this wave.

## Reason

The owner, 2026-10-30: "be adopted by b10x chef - ask it". `b10x-chef-orchestrator` answered on
2026-10-30, under the grant in the workspace instructions: "Adopted. Under the grant in
~/beyond10x/CLAUDE.md, this is a class-C item for wave 2026-10-30-w01: one bot PR, auto-merge to
main, and release 0.17.0 through llm-gateway's own process." On the store move: "PR 34 (D243-G1:
credential-namespace manifests and guide, no release) merged in this repository today.
story:credential-namespace-gateway is active. Its move to implemented is a store-only commit; let it
ride your wave. G2, the service-account token verifier behind OwnerVerifier, folds later into
story:per-run-client-tokens and is not in this wave."

## What the operator would have been asked

Whether the wave's pull request, merge and release may proceed; the orchestrator answered.

## Limits

One full gate on the integration branch when every unit is in (CI's gate on the pull request where
disk is short). Builds stop under 35G free. `verify-release.sh llm-gateway 0.17.0 <announcement>`
exits 0 before the release is reported. The five stale 2026-10-08 branches stay.
