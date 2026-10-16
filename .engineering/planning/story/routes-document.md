---
format: aep.planning-md/3
id: story:routes-document
kind: story
status: active
title: A deployment may serve the models of an llm.routes/1 document
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-translation
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: contracts/gateway/scenarios
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
  path: crates/llm-gateway-cli/src/routes.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/trusted.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/upstream.yaml
- confidence: inferred
  path: website/docs
- confidence: inferred
  path: website/docs/reference/deployment-document.md
revision: 21
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T16:32:04Z", actor: "human:timo", revision: 20}
- {from: "proposed", to: "active", at: "2026-10-16T16:32:05Z", actor: "human:timo", revision: 21}
---
## Outcome

The deployment document may name an llm `llm.routes/1` document, `routes = "<path>"`. Each route
becomes a served model alias, listed by `GET /v1/models` with the wires it accepts. The route's
endpoint decides the upstream wire, base URL, authentication and credential. A request on the
endpoint's own wire is relayed byte for byte to it (the hosted endpoint derived from the route's
endpoint, with the base-path rule of `story:hosted-base-path`); a request on another wire is
translated (`story:gateway-translation`).

## Context

Milestone 2 part B of the any-wire proxy plan (operator-approved 2026-10-16). llm 0.6.0 ships
`llm_models::routes::Routes` (crate `b10x-llm-models`), which parses and validates the document
strictly and performs no I/O. `[models.*]` (Runpod) and `[hosted.*]` keep working beside routes.

## Rules

1. `routes` is an optional string path in the deployment document's top level. The file is read
   once at start through the trusted-file reader (refused when group- or world-writable, bounded)
   and parsed with `Routes::parse`; a document llm refuses is `config:value` naming `routes`.
2. Credentials come only from trusted files, as every other credential of the gateway: an
   endpoint's `file:` secret is read once at start through the trusted-file reader with the
   endpoint-credential policy (`~/` resolved against `HOME`); an `env:` or `keychain:` secret is
   refused `config:value` naming `routes.<endpoint>.auth.secret`.
3. A route name that is also a `[models.*]` or `[hosted.*]` alias is `config:value`.
4. The endpoint's `base_url` must name its API path (llm appends the wire path to it, the gateway's
   base-path rule replaces the wire path's leading `/v1`; both agree only when the URL has a path):
   a pathless `base_url`, or a path outside the base-path rule, is `config:value`. A credential
   over `http` to a non-loopback host is refused as for `[hosted.*]`.
5. The route's capabilities (llm's declared or default ones) bound the translated request; `tools`
   decides the model's tool calling; `context_window` is the listed window.
6. The inventory lists one route per route name, its target naming the endpoint's protocol, and
   never its URL, credential file or credential.

## Acceptance

1. A deployment naming a routes document with two endpoints (a Messages and a Chat Completions
   loopback fixture) serves both route names: same-wire requests are relayed byte for byte to the
   endpoint's base path, cross-wire requests are translated (conformance scenario
   `routes-*` and CLI tests).
2. `GET /v1/models` lists each route with all three accepted wires and its upstream wire.
3. An `env:` or `keychain:` secret, a pathless `base_url`, a route name equal to a hosted alias,
   and a document llm refuses each start nothing with `config:value` (CLI tests).
4. A `file:` secret is read through the trusted reader: a group-readable credential file is
   refused `endpoint-credential:*` (CLI test).
5. The same deployment also serving a `[hosted.*]` model serves it unchanged beside the routes,
   and `GET /v1/routes` names each route's protocol and none of its URL, credential file or
   credential (CLI test or scenario).

## ESS first

`spec/domains/deployment.yaml` gains the `routes` key and a `RoutesDeclaration`;
`spec/domains/upstream.yaml` says a route's endpoint is a `HostedEndpoint`. Validated with
`ess specify validate --path spec` before code.

## Scope

Planned 2026-10-16 for wave 2026-10-16-w02 (inferred until the unit confirms):

- `crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/routes.rs` (new),
  `crates/llm-gateway-cli/src/hosted.rs`, `crates/llm-gateway-cli/src/serve.rs`,
  `crates/llm-gateway-cli/src/relaying.rs`, `crates/llm-gateway-cli/src/trusted.rs`,
  `crates/llm-gateway-cli/Cargo.toml`
- `checks/conformance/src/gateway.rs`, `contracts/gateway/scenarios/routes-*.yaml`
- `spec/domains/deployment.yaml`, `spec/domains/upstream.yaml`
- `website/docs/reference/deployment-document.md`, the translation page, `docs/gateway.md`,
  `CHANGELOG.md`, `README.md`

Shares `relaying.rs`, `hosted.rs`, `checks/conformance/src/gateway.rs` and the specification with
`story:gateway-translation`, so it runs after it in the same wave.

## Not in scope

Route fallback chains and routes without a model (llm routes are single-target); `env:` and
keychain secrets; reloading the routes document while serving.
