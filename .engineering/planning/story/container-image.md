---
format: aep.planning-md/3
id: story:container-image
kind: story
status: active
title: llm-gateway ships a container image definition and the proven model profiles
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: Dockerfile
- confidence: inferred
  path: crates/llm-gateway-cli/tests/profiles.rs
- confidence: inferred
  path: docs/model-profiles.md
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-08T14:33:53Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":4}}}
---
## Outcome

llm-gateway ships a container image definition (row D1). It also ships llmgw's two proven model
profiles and its weight-cache block from `docs/model-profiles.md`, written as blocks of this
repository's deployment document (row D3).

## Rows

D1, D3, from `docs/llmgw-capability-matrix.md`. Split out of `story:gateway-deployment` on
2026-10-06.

## Acceptance

1. A root `Dockerfile` builds `b10x-llm-gateway` from this workspace. A test parses the file and
   checks five things, each matching row D1:
   - the final stage's base image is distroless;
   - the user is non-root;
   - `EXPOSE 8080` is present;
   - the entrypoint is `b10x-llm-gateway`;
   - a label carries the source revision.

   Building the image is not part of `task check`. A build is recorded once in a
   `docs/verification/` record, with the command and its exit status.
2. `docs/model-profiles.md` holds three blocks. Two are llmgw's profiles: "Default: broad
   availability, 32k context" and "High throughput: H100 NVL with MTP speculative decoding, 64k
   context". The third is the weight-cache block (llmgw `docs/model-profiles.md:8`, `:44`, `:88`,
   at `048ebd8`).
3. A test loads each of the three through the binary's own document parser. The two profiles are
   each accepted as a model declaration with the context length the profile's title names. The
   weight-cache block is accepted when added to either profile.
4. Rows D1 and D3 are `covered` in the matrix.

## Scope (inferred)

`Dockerfile`, `docs/model-profiles.md`, `crates/llm-gateway-cli/tests/profiles.rs` (new).
