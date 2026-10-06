---
format: aep.planning-md/3
id: story:provider-key-files
kind: story
status: draft
title: The binary reads the Runpod API key and the vLLM key from trusted files
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-spec-declarations
- depends_on: story:spec-diff-gate
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/trusted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: spec/domains/deployment.yaml
revision: 9
---
## Outcome

The deployment document names two kinds of key file: the Runpod API key file (row K7) and, per
model, the file holding the key its pod's vLLM server expects (row B9). At startup the binary
reads each file once through the trusted-file reader. It holds the values as redacted secrets
and refuses a file that breaks a trusted-file rule with `<source>:<rule>`, as it does for
`owner_secret_file`.

## Rows

K7, B9, from `docs/llmgw-capability-matrix.md`. Split out of `story:gateway-deployment` on
2026-10-06, because its scope covers four independent slices (`story-scoper`).

## Decided

The vLLM key is read from a file and not derived. llmgw derives it from the Runpod key by
SHA-256. Here the operator supplies the same value they stored as the model's Runpod secret: the
pod receives `{{ RUNPOD_SECRET_<api_key_secret> }}` (`crates/llm-runpod/src/request.rs:116-117`),
and that pod-side reference does not change. Decided 2026-10-06 in the replan, because a file is
what the matrix row asks for and it keeps the two keys independent. This story records it as
`DECIDED` in `spec/domains/deployment.yaml`.

## ESS first

`spec/domains/deployment.yaml` declares the new keys and the new `StartupRefusal` variants before
any code, and replaces its `DEFERRED:` note on K7.

## Acceptance

1. A document with `runpod_api_key_file` and a model's `vllm_api_key_file` starts. Neither file's
   bytes appear in any response or in any line on standard error. A process test checks this by
   searching every byte written.
2. Each trusted-file rule broken by either file refuses startup, with `runpod-api-key:<rule>` or
   `vllm-api-key:<rule>`. Each is a row-named process test beside the `k30_*` owner-secret
   refusals.
3. After startup the binary holds both values, and their `Debug` prints neither. A unit test in
   `crates/llm-gateway-cli` checks the values it read and the redaction.
4. The keys are optional in the document. A document without them still starts, so profiles
   written without keys keep parsing.
5. `crates/llm-gateway-cli/tests/config.rs` no longer expects `runpod_api_key_file` to be refused,
   and rows K7 and B9 are `covered` in the matrix.

The pod sending the key as a bearer is `story:gateway-deployment`'s acceptance. Handing the
Runpod key to the production transport is `story:live-runpod-wiring`'s.

## Depends on

`story:gateway-spec-declarations`. Both stories change `spec/domains/deployment.yaml`, and this
story's new keys and refusals are declared on top of the deployment surface that story declares.

## Scope (inferred)

`crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs`,
`crates/llm-gateway-cli/src/trusted.rs`, `crates/llm-gateway-cli/src/refusal.rs`,
`crates/llm-gateway-cli/tests/config.rs`, `crates/llm-gateway-cli/tests/binary.rs`,
`spec/domains/deployment.yaml`.
