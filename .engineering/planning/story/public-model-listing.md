---
format: aep.planning-md/3
id: story:public-model-listing
kind: story
status: implemented
title: GET /v1/models and GET / answer without a credential and wake no pod
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-spec-declarations
- depends_on: story:provider-key-files
- informed_by: epic:client-access
- depends_on: story:model-tool-calling
scope:
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/relaying.rs
- confidence: cited
  path: crates/llm-gateway/src/listing.rs
- confidence: cited
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_public_listing.rs
- confidence: cited
  path: crates/llm-gateway/tests/gateway.rs
- confidence: cited
  path: crates/llm-gateway/tests/public_listing.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/clients.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
revision: 19
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T20:01:24Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"review_outcome":12}}}
- {from: "proposed", to: "active", at: "2026-10-08T20:01:24Z", actor: "human:timo", revision: 16, decided_on: {"recorded":{"review_outcome":12}}}
- {from: "active", to: "implemented", at: "2026-10-08T21:43:29Z", actor: "human:timo", revision: 19, decided_on: {"recorded":{"test_result":1,"review_outcome":13,"verification":1}}}
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

## Scope (confirmed by the implementor, wave 2026-10-08-w06)

Confirmed: `crates/llm-gateway/src/server.rs`, `docs/gateway.md`, `spec/domains/clients.yaml`,
`spec/domains/gateway.yaml`, `checks/conformance/src/gateway.rs`.

Corrected: `crates/llm-gateway/src/inventory.rs` was not touched; the rendering is in the new
`crates/llm-gateway/src/listing.rs`, and `relay.rs` and `metrics.rs` changed too. The binary's
change is `relay_models` in `crates/llm-gateway-cli/src/relaying.rs`, not
`crates/llm-gateway-cli/src/serve.rs`. `crates/llm-gateway/tests/gateway.rs` only gains
`listing.rs` in `SOURCES`; the cases are in `crates/llm-gateway/tests/public_listing.rs`,
`crates/llm-gateway/tests/adversary_public_listing.rs` and
`crates/llm-gateway-cli/tests/relaying.rs`.

## Client profiles

Added 2026-10-08 for `epic:client-access` (`docs/design/runpod-clients.md` § 4, § 5). These are
acceptance criteria, numbered on from the list above.

9. The Codex and Claude Code answers of `GET /` (criterion 2) carry, per model, the settings of
   that client's `llm-gateway.clients.ClientProfile` (`spec/domains/clients.yaml`), with the
   model's alias and `context_window` as their values. A row-named test reads each setting name
   and each value out of the answer for a document with two models.
10. The plain-text and HTML answers carry the Loom profile: the llm catalog lines of design § 5
    step 7, per model, for the `chat` wire, with alias and `context_window` filled in. A test reads
    them out of the plain-text answer, and a second test out of the HTML answer.
11. A model that does not declare the client's wire, or whose tool calling is `Absent`, gets no
    profile for that client. Two tests, one per condition, find the model's alias absent from that
    client's profile.
12. The `UNMAPPED` marker on `ClientProfile` in `spec/domains/clients.yaml` is replaced by a
    `DECIDED` line, and `ess specify validate --path spec` passes.

Depends on `story:model-tool-calling`, which declares the tool calling criterion 11 reads; both
change `docs/gateway.md`, `spec/domains/clients.yaml`, `crates/llm-gateway-cli/src/serve.rs` and
`crates/llm-gateway/tests/gateway.rs`.

