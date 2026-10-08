---
format: aep.planning-md/3
id: story:hosted-endpoints
kind: story
status: draft
title: A relayed model can be served by a hosted endpoint, not only a Runpod pod
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
- depends_on: story:cold-start-hold
- depends_on: story:gateway-observability
- depends_on: story:pod-proxy-tls
- depends_on: story:claude-code-wire
scope:
- confidence: inferred
  path: checks/conformance/Cargo.toml
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/trusted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/support/mod.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
revision: 25
---
## Outcome

A relayed model can be served by a hosted endpoint declared in the deployment document: any
server that speaks the model's wire at an `http` or `https` base URL, authenticated with a
credential read from a file through the trusted-file reader. A Runpod pod remains the other way
to serve a model.

## Why

`runpod-vllm` is the only provider kind the deployment document admits
(`llm-gateway.deployment.ProviderKind`). Every gateway in the 2026-10-06 research reaches hosted
APIs as well (`epic:gateway-features`, row "Hosted provider endpoints").

## ESS first

`llm-gateway.upstream.HostedEndpoint` in `spec/domains/upstream.yaml` is marked `PLANNED`. This
story adds its observation entity and scenarios, and settles the marker `UNMAPPED: how the closed
deployment document declares an endpoint` in the specification before any code.

## Acceptance

Cases 2, 3, 4 and 7 are `Relay` conformance scenarios, and case 1 is an `Exchange` inspection
scenario. Cases 5 and 6 are startup refusals, and they are proved the way the binary's other
refusals are: as row-named process tests in `crates/llm-gateway-cli/tests/`, because the
conformance targets do not model files or processes (`spec/domains/deployment.yaml`,
`ESS-LIMIT`). Every case runs against a loopback fixture endpoint, and none makes a paid call.

1. A deployment document that declares a hosted endpoint for a model starts. `GET /v1/routes`
   lists that model's target with its `auth_kind` and `billing_kind`. Neither the base URL nor
   any byte of the credential file appears in any response, which the test checks by searching
   every byte written.
2. A relayed request for that model reaches the fixture at the declared base URL, on the same
   path. A `bearer` endpoint receives `authorization: Bearer <credential>`. An `api-key`
   endpoint receives the credential as the value of the header its `credential_header` names,
   and no `authorization` header. An `anonymous` endpoint receives neither.
3. The owner's bearer credential is absent from every byte any fixture receives.
4. An `https` endpoint is reached over TLS. A fixture whose certificate the gateway does not
   trust ends the request `upstream-failed`, and no request byte reaches it.
5. A credential file that breaks a trusted-file rule refuses startup with
   `endpoint-credential:<rule>`, using the rules `owner_secret_file` follows
   (`llm-gateway.deployment.StartupRefusal`).
6. The document is refused as `config:value` in two cases: an endpoint whose `auth_kind` is not
   `anonymous` but which declares no credential file, and an `api-key` endpoint that names no
   `credential_header`.
7. A model served by a Runpod pod behaves exactly as before: the `Relay` scenarios in
   `contracts/gateway/scenarios/` still pass.

## Depends on

- `story:gateway-deployment`. It adds the relay's upstream credential header (row B8: the
  vLLM key sent to the pod). This story reuses that header for hosted endpoints rather than adding
  a second one. Both stories change `crates/llm-gateway-cli/src/config.rs` (`ProviderKind`,
  `Provider`) and `crates/llm-gateway-cli/src/serve.rs` (the target source).
- `story:cold-start-hold`. It edits the same deployment document (row K28) and the same
  `RelayTargets` port (`crates/llm-gateway/src/relay.rs:86-94`).
- `story:gateway-observability`. It adds the per-call record and its conformance observation in
  `crates/llm-gateway/src/relay.rs` and `checks/conformance`, which this story's scenarios also
  change.

## Scope (inferred)

Superseded by `## Scope`, which `story-scoper` derived on 2026-10-06. The typed entries are in the
frontmatter `scope`.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-gateway-cli`: the deployment document, the trusted-file reader, startup refusals, and the target source that opens each endpoint's connection, TLS included. Cited (story "Depends on"; `crates/llm-gateway/src/relay.rs:5-7`)
- **File:** `crates/llm-gateway-cli/src/config.rs`. Cited: `ProviderKind` `:127-130`, `Provider` `:155`. `ModelDocument` `:63-97` requires `hf_model`, `image`, `gpu_types` and `max_model_len`, none of which a hosted model has
- **File:** `crates/llm-gateway-cli/src/serve.rs`. Cited: `inventory` hard-codes `AuthKind::Bearer` and `BillingKind::SelfHosted` at `:104-105`; `start` binds without a relay at `:166`
- **File:** `crates/llm-gateway-cli/src/refusal.rs`. Cited: case 5's `endpoint-credential:<rule>` needs a new `Source` (`:7-10`) and new variants. The test at `:165` admits only four sources, and the test at `:142-150` fails unless the variants equal `deployment.yaml`'s
- **File:** `crates/llm-gateway-cli/src/trusted.rs`. Cited: a third `Policy` beside `CONFIG` `:29` and `OWNER_SECRET` `:39`
- **File:** `spec/domains/upstream.yaml`. Cited: `HostedEndpoint` `:53-83` and its `UNMAPPED` marker `:80-82`
- **File:** `spec/domains/deployment.yaml`. Cited: `StartupRefusal` `:58-60`, `ProviderKind` `:66-68`
- **File:** `checks/conformance/src/gateway.rs`. Cited: the `Relay` program `:512-516` declares no endpoint, `Target::connect` opens plain TCP at `:846`, and fixture pods record no request header
- **File:** `contracts/ess-inputs.yaml`, `contracts/suite.json`, `contracts/schema/schema/types/llm-gateway.deployment.StartupRefusal.schema.json`. Cited: new scenarios and regeneration
- **File:** `crates/llm-gateway-cli/Cargo.toml`. Inferred: the TLS client crate goes here; `Cargo.lock` holds no TLS crate today
- **File:** `Cargo.lock`. Inferred: follows the TLS dependency
- **File:** `crates/llm-gateway-cli/src/lib.rs`. Inferred: registers and exports the hosted target-source module
- **File:** `crates/llm-gateway/src/relay.rs`. Inferred: the request head at `:582-587` has no credential line. An `api-key` header name, or a base URL with a path prefix, needs a change here unless `story:gateway-deployment`'s header already carries a name and a value
- **File:** `crates/llm-gateway-cli/tests/binary.rs`. Inferred: case 5 goes beside the `k30_*` owner-secret refusals `:507-575`
- **File:** `crates/llm-gateway-cli/tests/config.rs`. Inferred: case 6 goes beside `k29_every_value_outside_its_rule_is_refused` `:368`
- **File:** `crates/llm-gateway-cli/tests/support/mod.rs`. Inferred: the fixture document writer `document` `:70`
- **File:** `checks/conformance/Cargo.toml`. Inferred: a TLS fixture server for case 4, or a dependency on `b10x-llm-gateway-cli` so the scenarios drive the binary's own target source
- **File:** `spec/domains/gateway.yaml`. Inferred: the `Relay` program grammar `:202-222` and `RelayObservation` `:160-188` carry no endpoint and no received header, unless the new observation entity sits wholly in `upstream.yaml`
- **File:** `contracts/baseline.json`, `contracts/schema/schema/entities/llm-gateway.upstream.HostedEndpoint.schema.json`. Inferred: floors and regeneration
- **Symbols:** `ProviderKind`, `Provider`, `ModelDocument`, `StartupRefusal`, `Source`, `trusted::Policy`, `RelayTarget::connect`, `llm-gateway.upstream.HostedEndpoint`. Cited
- **Not touched:** `crates/llm-gateway/src/inventory.rs`. Cited: `AuthKind` `:60-64` and `BillingKind` `:78-82` already have every variant case 1 reports
- **Documents:** `README.md` (cited: `:100` "Both files go through a trusted-file reader", and the credential file makes three); `docs/gateway.md` (inferred: the relay contract `:115-124` changes if the request head gains a credential header); `AGENTS.md` (inferred: its `spec/` row says no code implements `llm-gateway.upstream`)
- **Confidence:** medium. The CLI, spec and refusal files come from the story and the tree. The TLS crate, the conformance harness shape, `relay.rs` and the new file names depend on decisions nobody has made yet
- **Would collide with:** any unit on the CLI deployment document or target source (`config.rs`, `serve.rs`); any unit that adds a startup refusal (`refusal.rs`, `deployment.yaml`); any unit on the relay request head or the `RelayTarget`/`RelayTargets` port (`relay.rs:67-94`, `:581-595`); any unit on the `Relay` conformance program or observation (`checks/conformance/src/gateway.rs`); any unit that adds scenarios or regenerates `contracts/`; any unit that adds a dependency (`Cargo.lock`)
- **Safety fact:** TLS and the endpoint credential can stay out of the gateway crate's dependency closure. `RelayTarget::connect` returns any `Read + Write + Send` stream (`relay.rs:67-83`), and `dependency_boundary.rs` holds that crate to zero declared dependencies and a closure of itself. Also, `relay.rs:581-591` writes no request byte until `connect` returns `Ok`, so a handshake done inside `connect` meets case 4's "no request byte reaches it". Step 2, unproven

Not established: which TLS crate (`rustls` with ring or aws-lc-rs, or `native-tls`); how case 4's fixture gets a trusted root (`HostedEndpoint` has no CA field, so a CA file in the document or a test-only root); whether the `Relay` scenarios drive the binary's own target source; the `UNMAPPED` declaration shape.

## TLS and ordering

Added 2026-10-08 (`epic:client-access`, design critic round 1). This section supersedes the TLS
lines of `## Scope` below: the TLS crate in `crates/llm-gateway-cli/Cargo.toml`, "which TLS
crate", and the open question of a CA file in the document or a test-only root.

This story reuses the binary's TLS client and its test-root seam from `story:pod-proxy-tls`. The
shipped binary takes no trust-root key in the document (`story:pod-proxy-tls` acceptance 3), so
case 4's fixture gets its root through that seam. This story adds no TLS dependency of its own.

It also comes after `story:claude-code-wire`: both change `crates/llm-gateway/src/relay.rs` and
`docs/gateway.md`.
