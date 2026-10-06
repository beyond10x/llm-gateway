---
format: aep.planning-md/3
id: story:public-model-listing
kind: story
status: draft
title: GET /v1/models and GET / answer without a credential and wake no pod
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-spec-declarations
- depends_on: story:provider-key-files
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway/src/inventory.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: cited
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/gateway.yaml
revision: 10
---
## Outcome

Two routes are answered without a credential. `GET /v1/models` lists the models in the OpenAI
list shape, with `max_model_len` and `wires` (row R5). `GET /` answers setup instructions chosen
by `User-Agent` (row R1). Neither route wakes a pod. Neither renders any owner-only fact: no
provenance, no `config_digest`, no endpoint, no credential.

## Rows

R1, R5, from `docs/llmgw-capability-matrix.md`. Split out of `story:gateway-deployment` on
2026-10-06.

## ESS first

`spec/domains/gateway.yaml` declares the two routes, the listing's fields and the answers to other
methods before any code.

## Acceptance

1. `GET /v1/models` without a credential answers 200 with one entry per model: alias,
   `max_model_len` and `wires`.
2. `GET /` without a credential chooses its answer from the lowercased `User-Agent`, by the rules
   of llmgw `src/lib.rs:231-245` at `048ebd8`:
   - containing `codex`: the Codex `config.toml` profile, as text;
   - containing `claude` or `anthropic`: the Claude Code environment variables, as text;
   - starting with `mozilla/`: an HTML page;
   - anything else, or no `User-Agent`: plain text.

   Each of the four is a row-named test that sends one such value: `codex-cli/1.0`,
   `claude-cli/2.0`, `Mozilla/5.0` and `curl/8.0`.
3. For both routes, a test searches every byte of the answer. It finds no provenance label, no
   `config_digest`, no endpoint identifier and no byte of the owner secret.
4. Neither route calls the target source. A test counts acquisitions and finds zero.
5. Only `GET` and `HEAD` match the two routes without a credential, as for the probes. A `POST`
   to either route without a credential answers `credential-absent`. With the owner credential it
   answers `method-not-allowed`. Both cases are tests.
6. `GET /v1/routes` still requires the owner credential, and its existing scenarios pass unchanged.
7. In `docs/gateway.md`, two sentences change. The one at `:86-87`, that "every response the
   gateway writes itself is `application/json`", names the text and HTML answers of `GET /` as
   the exception. The one at `:100`, that the unauthenticated surface is "a closed set of two
   literal paths", lists four paths. The tests that `include_str!` the document pass.
8. Rows R1 and R5 are `covered` in the matrix.

## Depends on

- `story:gateway-spec-declarations`. Both stories change `spec/domains/gateway.yaml`,
  `docs/gateway.md` and `checks/conformance/src/gateway.rs`. This story declares new routes on
  top of the surface that story declares.
- `story:provider-key-files`. Both change `crates/llm-gateway-cli/src/serve.rs`. The listing's
  `max_model_len` is held only by the binary's model (`crates/llm-gateway-cli/src/config.rs:450`),
  so `inventory()` and `start` in `serve.rs` change. The key files are read in the same module.

## Scope (inferred)

`crates/llm-gateway/src/server.rs`, `crates/llm-gateway/src/inventory.rs`,
`crates/llm-gateway-cli/src/serve.rs`, `docs/gateway.md`, `crates/llm-gateway/tests/gateway.rs`,
`spec/domains/gateway.yaml`, `checks/conformance/src/gateway.rs`.
