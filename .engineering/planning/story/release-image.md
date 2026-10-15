---
format: aep.planning-md/3
id: story:release-image
kind: story
status: draft
title: Every release publishes the gateway image to ghcr.io/beyond10x/llm-gateway
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

Every release publishes the gateway's container image, built from the root `Dockerfile` at the
release tag, as `ghcr.io/beyond10x/llm-gateway:<version>`, so a cluster can run a released
gateway without building it.

## Acceptance

1. A workflow `.github/workflows/release.yml` runs on a pushed tag of the form `<major>.<minor>.<patch>`
   and on nothing else. It builds the root `Dockerfile` natively for `linux/amd64` and
   `linux/arm64` (no emulation), pushes one image per architecture and one manifest list
   `ghcr.io/beyond10x/llm-gateway:<version>`, and passes `SOURCE_SHA` as the tag's commit, so the
   image's `org.opencontainers.image.revision` label names it. Its permissions are
   `contents: read` and `packages: write`, nothing more; every action it uses is pinned by commit
   SHA.
2. The workflow refuses to replace a tag that already exists in the registry: a second run for the
   same version fails before pushing and says so.
3. A test (`crates/llm-gateway-cli/tests/release_workflow.rs`, beside `tests/dockerfile.rs`) parses
   the workflow and fails when the trigger, the image name, the two platforms, the permissions,
   the revision argument or an unpinned action differs from 1.
4. `AGENTS.md` § Releases no longer says there is no release workflow: it names the workflow, the
   image and the check that the image exists for the tag (`docker manifest inspect` or the
   packages API) as part of verifying a release.
5. The first release after this story records the published image digest in a
   `docs/verification/` record.

## Not in scope

Signing, SBOM or provenance attestation; a `latest` tag; a Helm chart.

## Scope (inferred)

`.github/workflows/release.yml` (new), `crates/llm-gateway-cli/tests/release_workflow.rs` (new),
`AGENTS.md`, `CHANGELOG.md`.
