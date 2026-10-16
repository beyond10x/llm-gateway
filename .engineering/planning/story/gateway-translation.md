---
format: aep.planning-md/3
id: story:gateway-translation
kind: story
status: active
title: The gateway translates only the supported protocol subset
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- informed_by: story:modal-hosting
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: contracts/gateway/scenarios
- confidence: inferred
  path: contracts/gateway/scenarios/k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target.yaml
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: inferred
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway/src/listing.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/src/translate.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
- confidence: inferred
  path: website/docs
revision: 33
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T14:31:57Z", actor: "human:timo", revision: 18, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "proposed", to: "active", at: "2026-10-16T16:32:04Z", actor: "human:timo", revision: 33, decided_on: {"recorded":{"review_outcome":4}}}
---
## Outcome

A client may speak any of the three wires (`POST /v1/chat/completions`, `/v1/responses`,
`/v1/messages`) to any served model. When the model's upstream declares the client's wire the
gateway relays the bytes as it does today; when it does not, the gateway translates through llm's
neutral turn: it decodes the client's body with llm's ingress decoder, sends the turn to the
upstream with llm's client for the upstream's wire, and writes the answer back on the client's
wire with llm's ingress encoder. The model's declared `wires` now means "the upstream speaks
these"; translation is automatic and replaces `wire-not-served` in the binary.

## Context

Milestone 2 part B of the any-wire proxy plan (operator-approved 2026-10-16). Part A (PR 15)
added hosted base paths, static headers and optional ingress auth. llm 0.6.0
(https://github.com/beyond10x/llm/releases/tag/0.6.0, commit 74921f7c) ships the three ingress
decoders and encoders this story uses. Publish exact supported ingress semantics, not a claim of
complete vendor API equivalence. No downstream tool execution.

## Design (the plan's answers)

1. **Where.** A std-only port in `crates/llm-gateway` (`Translator`, `Translation`): `relay::serve`
   calls it when the ingress wire is not among the model's upstream wires, instead of refusing
   `wire-not-served`. A relay composed without a translator keeps `wire-not-served`, so the
   library and `tests/dependency_boundary.rs` are unchanged in what they link. The CLI implements
   the port with llm 0.6.0 crates (taken by tag) and a tokio runtime the CLI owns.
2. **Decode before any target.** The client's body is decoded and checked against the model's
   capabilities before the target source is asked, so a refused request never wakes a pod.
3. **Upstream.** The target the source hands out (a pod, a hosted endpoint) supplies the scheme,
   authority, base path and credential; the CLI builds the llm binding for the upstream wire from
   it per request and runs one turn. The gateway's own static headers are not sent on the
   translated path (llm's clients send their own protocol headers).
4. **Opaque state.** Ingress carries reasoning state it cannot attribute as
   `Item::UnattributedOpaque`. On the translated path the upstream speaks another protocol than
   the one the state was read from, so `TurnRequest::bind_unattributed` would refuse it anyway;
   the gateway never binds carried state and refuses such a request before dispatch.
5. **Refusals.** Unsupported fields and opaque state are refused before dispatch, never dropped:
   new codes `translation-unsupported`, `translation-invalid`, `translation-opaque-state`. An
   upstream that refuses the translated request before any output is `upstream-refused`; one that
   cannot be reached is `upstream-failed`, as on the byte relay.
6. **Answers.** A streamed answer is written as the client wire's server-sent events, event by
   event; a non-streamed one as its JSON object. When llm's encoder refuses the upstream answer
   (`story:ingress-block-boundaries` in llm: Responses two messages back to back or a message
   interrupted by reasoning or a call; Messages redacted thinking between streamed blocks), the
   client gets a wire-correct terminal error (`response.failed`, an `error` event, or a Chat
   `data: {"error":…}` chunk without `[DONE]`), or, non-streamed, the refusal
   `translation-unrepresentable`; never a truncated or malformed stream.
7. **Usage.** The upstream's reported counters are carried into the translated answer by the
   encoders; an unreported counter stays unreported. Usage records and metrics stay
   `story:usage-records`.
8. **Cancellation.** A client that stops reading cancels the turn (llm `Cancel`); nothing is
   replayed after possible dispatch.
9. **Messages rewrites.** The effort mapping and the `max_tokens` clamp through `count_tokens`
   are vLLM/Qwen-specific and stay on the native Messages path; the translated path applies
   neither (the projection writes the upstream model and llm's own fields).
10. **Listing.** `GET /v1/models` lists, per model, the wires it accepts (all three when the relay
    has a translator) and the upstream wires it relays byte for byte.

## Acceptance

Conformance scenarios in `contracts/gateway/scenarios/translate-*.yaml` over loopback fixture
upstreams, plus CLI tests; no paid provider call:

1. The 3×3 matrix (ingress chat/responses/messages × upstream chat/responses/messages): the six
   cross pairs translate a streamed text answer with usage, and the three same-wire pairs are
   relayed byte for byte as before.
2. A tool call from the upstream reaches the client as that wire's tool call; a tool-result turn
   from the client reaches the upstream as that wire's tool result.
3. A request carrying a field outside llm's subset is `400 translation-unsupported`; one carrying
   opaque reasoning state is `400 translation-opaque-state`; neither asks the target source for a
   target.
4. An upstream answer the encoder refuses ends in a wire-correct terminal error event.
5. A client that disconnects mid-stream cancels the upstream turn (the fixture upstream sees its
   connection closed before its script ends).
6. `GET /v1/models` lists all three wires and the upstream wires per model.
7. `k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target` still holds for a
   relay composed without a translator.
8. The supported subset is published, not a claim of vendor API equivalence: `docs/gateway.md` and
   a site page on translation name, per ingress wire, the request fields llm 0.6.0 accepts, the
   behaviour per (ingress, upstream) pair, the refusal codes, and the known limits (fields Codex
   and Claude Code send that the subset refuses, the encoder refusals); the page's statements
   about refused fields are each backed by a test that feeds the request to the decoder.
9. A non-streamed Chat or Messages request is answered with that wire's JSON object; a translated
   request reaching a Messages upstream carries no effort mapping and no `count_tokens` call
   (the fixture upstream's received requests show it); a request breaking a subset rule is
   `400 translation-invalid`.

## Verification

`task check` (Rust 1.98.0) green; conformance answered/total rise above 232; the matrix scenarios
named in the evidence. A passing scaffold build is not runtime evidence. No paid call runs in the
gate.

## Scope

Planned 2026-10-16 for wave 2026-10-16-w02 (all inferred until the unit confirms):

- `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/src/translate.rs` (new port),
  `crates/llm-gateway/src/error.rs`, `crates/llm-gateway/src/lib.rs`,
  `crates/llm-gateway/src/listing.rs`, `crates/llm-gateway/src/server.rs`
- `crates/llm-gateway-cli/Cargo.toml`, `crates/llm-gateway-cli/src/translate.rs` (new),
  `crates/llm-gateway-cli/src/relaying.rs`, `crates/llm-gateway-cli/src/hosted.rs`,
  `crates/llm-gateway-cli/src/lib.rs`
- `checks/conformance/src/gateway.rs`, `contracts/gateway/scenarios/translate-*.yaml`
- `spec/domains/gateway.yaml`, `spec/domains/upstream.yaml`
- `docs/gateway.md`, `website/docs/` (a translation page), `crates/llm-gateway-docs/src/status.rs`,
  `CHANGELOG.md`

## Opaque state carried from ingress

Carried from wave 3, `review-result:adversary-opaque-pass-2` finding 2: a gateway that binds every
carried entry to the binding that read the request reinstates the laundering
`story:unattributed-opaque-state` closed. Decided above (design 4): the translated path binds
nothing and refuses carried state before dispatch; the byte relay never decodes it.

## Moved

Moved from `beyond10x/llm` (`story:gateway-translation`, llm `8d8e752d`) by story:serving-extraction
on 2026-10-05. Its lifecycle history and evidence records stay in llm's store at
`.engineering/evidence/story/gateway-translation/`.

## Depends on

Replan 2026-10-16 (Milestone 2, operator-approved): translation carries usage inside the
translated answer, usage records stay their own story (`story:usage-records`), and fallback is not
required (`story:target-fallback`); both `depends_on` edges were removed. `story:modal-hosting` is
related as `informed_by`.

## Not in scope

Loading models from an `llm.routes/1` document (`story:routes-document`); usage records and token
metrics (`story:usage-records`); widening llm's ingress subset (llm's own stories).
