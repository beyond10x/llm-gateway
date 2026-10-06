---
format: aep.planning-md/3
id: story:gateway-translation
kind: story
status: draft
title: The gateway translates only the supported protocol subset
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:target-fallback
- depends_on: story:usage-records
- informed_by: story:modal-hosting
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: contracts/gateway/scenarios/k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target.yaml
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
revision: 17
---
## Context

Use shared projections/routing/costs and optional hosting. Publish exact supported ingress endpoints and semantics, not a claim of complete vendor API equivalence. No downstream tool execution. Apply deadlines and cleanup on client disconnect; test fallback visibility boundary.

## Acceptance

A three-protocol client/upstream test matrix preserves text, tools, streaming, cancellation and usage or explicitly refuses unsupported fields and opaque state.

## Evidence

Operator-approved design, 2026-09-19; docs/design.md; spec/system.yaml and spec/domains/catalog.yaml. Existing source references are listed under docs/design.md, Source evidence and draft limits.

## Verification

Retain commands and exact fixture/contract identities demonstrating the acceptance and its named failure cases. A passing scaffold build is not runtime evidence. No paid call runs in the normal gate.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong). It replaces the two crate- and directory-level lines this section held before.

- **Primary surface:** `crates/llm-gateway/src/relay.rs` — inferred: a model is served only to a target asked on the client's own path (`relay.rs:29`, `:563-565`, `:582-587`), which translation reverses; client-disconnect cleanup lands in `stream_answer` (`:626-676`)
- **Files:** `spec/domains/gateway.yaml` — cited: `:12-13` hands translation to this story as DEFERRED
- **Files:** `spec/domains/upstream.yaml` — cited: `:56-57` and `:76-79` name this story
- **Files:** `docs/gateway.md` — cited: `:6-7` leaves translation out, and AGENTS.md makes this document a contract that changes with the behaviour
- **Files:** `crates/llm-gateway/src/lib.rs` — cited: `:24` lists translation under what the crate does not do
- **Files:** `README.md` — cited: `:22` "it translates no protocol"
- **Also likely:** `crates/llm-gateway/src/body.rs` — inferred: the body is scanned, not parsed (`body.rs:1-10`), and translation needs a parse
- **Also likely:** `crates/llm-gateway/src/error.rs` — inferred: refusing unsupported fields and opaque state needs new `RefusalCode` variants
- **Also likely:** `crates/llm-gateway-cli/src/config.rs` — inferred: a model declares only the wires it serves (`config.rs:164`, `:315-323`), with no target wire
- **Also likely:** `crates/llm-gateway-cli/src/serve.rs` — inferred: it builds one target per declared wire (`serve.rs:91-92`)
- **Also likely:** `crates/llm-gateway-cli/Cargo.toml` — inferred: the one crate allowed to take llm's projection crates (see Safety fact)
- **Also likely:** `Cargo.lock` — inferred: any llm dependency taken by tag changes it
- **Also likely:** `checks/conformance/src/gateway.rs` — inferred: the matrix scenarios run through its `Relay` adapter (`:468-902`)
- **Also likely:** `contracts/gateway/scenarios/k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target.yaml` — inferred: it pins `wire-not-served`, which translation may replace
- **Also likely:** `contracts/ess-inputs.yaml`, `contracts/baseline.json`, `contracts/suite.json`, `contracts/schema/schema/commands/llm-gateway.gateway.Relay.schema.json` — inferred: new scenarios, floors and regeneration
- **Also likely:** `AGENTS.md` — inferred: its "Declares no dependency" row and its dependency-boundary rule change if translation goes into the library
- **Symbols:** `RelayModel`, `relay::serve`, `RefusalCode::WireNotServed` — inferred
- **Symbols:** `TurnRequest::bind_unattributed`, `Item::UnattributedOpaque` — cited: named by the story and defined in beyond10x/llm (`crates/llm-core/src/turn.rs:193`, `crates/llm-core/src/item.rs:41`), not in this tree
- **Confidence:** medium — inferred: the tree names this story at five places, but the story cites no file in this repository, and where the code goes depends on the dependency question below
- **Would collide with:** any unit changing `relay::serve` or `RelayModel` in `relay.rs` (story:target-fallback and story:usage-records claim it), the per-model wire declaration in `config.rs`/`serve.rs`, the `Relay` command in `spec/domains/gateway.yaml`, `HostedEndpoint` in `spec/domains/upstream.yaml`, `docs/gateway.md`, and any unit that adds a scenario — inferred
- **Safety fact:** the gateway library may link nothing (`crates/llm-gateway/tests/dependency_boundary.rs:22-33`, `:187-204`), and every llm crate behind "shared projections" pulls in a forbidden crate: llm-chat, llm-responses and llm-messages declare `b10x-llm-http`, `b10x-llm-credentials` and `tokio`, and llm-core reaches `tokio` through `tokio-util` (llm `Cargo.lock` at origin/main `d8ab3691`). So translation cannot use llm's projections inside `crates/llm-gateway` without changing that rule — step 2, unproven — inferred

Not established when the scope was derived: where the translation code lives (the library, the binary behind a port, or llm moving its projections out of its HTTP crates); whether translation is opt-in per model or replaces `wire-not-served`. Of the five `depends_on` edges llm's record held, none came across with the migration (the `## Depends on` section restores the one to `story:modal-hosting`): `story:ordered-fallback` and `story:unattributed-opaque-state` are implemented in beyond10x/llm, `story:gateway-auth` and `story:runpod-hosting` are implemented here, and `story:modal-hosting` is a draft here.

## Opaque state carried from ingress

Carried from wave 3, `review-result:adversary-opaque-pass-2` finding 2. Ingress now carries opaque
state it cannot attribute as `Item::UnattributedOpaque`, and binding it is the caller's explicit
decision (`TurnRequest::bind_unattributed`). A gateway that binds every carried entry to the binding
that read the request reinstates the laundering `story:unattributed-opaque-state` closed: a payload
minted under an earlier binding revision becomes sendable to the current one. The conformance
adapter does exactly that for its round-trip scenario (`checks/conformance/src/responses.rs:186-192`).
This story must decide which binding a gateway may bind carried state to, and refuse the rest.

## Moved

Moved from `beyond10x/llm` (`story:gateway-translation`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/gateway-translation/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.

## Depends on

- `story:target-fallback` and `story:usage-records`. This story's acceptance names the fallback
  visibility boundary and usage, which those two stories settle first. All three change
  `relay::serve` in `crates/llm-gateway/src/relay.rs`.

`story:modal-hosting` is related as `informed_by`, not `depends_on`. llm's record of this story
(llm `8d8e752d`) depended on it, but this story's Context calls hosting optional, and translation
needs nothing Modal supplies (replan, 2026-10-06). The other four edges in llm's record point at
stories that are implemented: `story:gateway-auth` and `story:runpod-hosting` here, and
`story:ordered-fallback` and `story:unattributed-opaque-state` in beyond10x/llm.
