---
format: aep.planning-md/3
id: story:provider-key-files
kind: story
status: implemented
title: The binary reads each model's vLLM key from a trusted file
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: README.md
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/keys.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/trusted.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/adversary_w02.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/support/mod.rs
- confidence: cited
  path: docs/llmgw-capability-matrix.md
- confidence: cited
  path: spec/domains/deployment.yaml
revision: 20
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T13:35:57Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-08T13:35:57Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-08T14:10:01Z", actor: "human:timo", revision: 20, decided_on: {"recorded":{"test_result":1,"review_outcome":5}}}
---
## Outcome

Per model, the deployment document names the file holding the key that model's pod's vLLM server
expects (row B9). At startup the binary reads each file once through the trusted-file reader. It
holds the values as redacted secrets and refuses a file that breaks a trusted-file rule with
`<source>:<rule>`, as it does for `owner_secret_file`.

The Runpod API key is not the binary's (row K7). Under design choice D1 = A
(`decision-blocker:runpod-control-plane`, `docs/design/runpod-clients.md` § 4) it lives in the
connectors keyring and the gateway never holds it. The document keeps refusing
`runpod_api_key_file`.

## Rows

B9, and K7 as moved, from `docs/llmgw-capability-matrix.md`. Split out of
`story:gateway-deployment` on 2026-10-06, because its scope covers four independent slices
(`story-scoper`). Rewritten 2026-10-08 for D1 = A: the design table says "the key file story reads
only the vLLM key" (`docs/design/runpod-clients.md`, D1 recommendation paragraph).

## Decided

The vLLM key is read from a file and not derived. llmgw derives it from the Runpod key by
SHA-256. Here the operator supplies the same value they stored as the model's Runpod secret: the
pod receives `{{ RUNPOD_SECRET_<api_key_secret> }}` (`crates/llm-runpod/src/request.rs:116-117`),
and that pod-side reference does not change. Decided 2026-10-06 in the replan, because a file is
what the matrix row asks for and it keeps the two keys independent; under D1 = A there is no
Runpod key in the gateway to derive it from. This story records it as `DECIDED` in
`spec/domains/deployment.yaml`.

## ESS first

`spec/domains/deployment.yaml` declares the per-model key and the new `StartupRefusal` variants
before any code, and replaces its `DEFERRED:` note on K7 with a `DECIDED` line saying the Runpod
API key is held by connectors, not by this document.

## Acceptance

1. A document whose model names a `vllm_api_key_file` starts. The file's bytes appear in no
   response and in no line on standard error. A process test checks this by searching every byte
   written.
2. Each trusted-file rule broken by the file refuses startup with `vllm-api-key:<rule>`. Each is a
   row-named process test beside the `k30_*` owner-secret refusals.
3. After startup the binary holds each model's value, and its `Debug` prints none. A unit test in
   `crates/llm-gateway-cli` checks the values it read and the redaction.
4. The key is optional in the document. A document without it still starts, so profiles written
   without keys keep parsing.
5. A document carrying `runpod_api_key_file` is still refused, and the test in
   `crates/llm-gateway-cli/tests/config.rs` that expects it names D1 = A as the reason.
6. Row B9 is `covered` in the matrix; row K7 says the key moved to connectors under D1 = A.

The pod receiving the key as a bearer is `story:gateway-deployment`'s acceptance.

## Depends on

Nothing open. `upstream-blocker:llm-runpod-provider` is cleared by llm 0.5.0; under D1 = A this
story needs no connectors release. The edges on `story:gateway-spec-declarations` and
`story:spec-diff-gate` were taken back on 2026-10-08: the first was a shared file
(`spec/domains/deployment.yaml`), which a wave orders by holding no other story that edits it; the
second only spared acknowledgements, and this story's change is additive.

## Scope (inferred)

`crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs`,
`crates/llm-gateway-cli/src/trusted.rs`, `crates/llm-gateway-cli/src/refusal.rs`,
`crates/llm-gateway-cli/tests/config.rs`, `crates/llm-gateway-cli/tests/binary.rs`,
`spec/domains/deployment.yaml`.
