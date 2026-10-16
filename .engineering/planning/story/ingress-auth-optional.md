---
format: aep.planning-md/3
id: story:ingress-auth-optional
kind: story
status: implemented
title: 'Ingress authentication for model calls is optional: ingress_auth = bearer or none'
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:hosted-static-headers
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: README.md
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/adversary_w02.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/hosted.rs
- confidence: cited
  path: crates/llm-gateway-docs/src/status.rs website/data/status.json website/docs/status.mdx
- confidence: cited
  path: crates/llm-gateway/src/auth.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_w10_ingress.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_w10_ingress_pass2.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/llmgw-capability-matrix.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
- confidence: cited
  path: website/docs/concepts/single-owner-gateway.md
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 35
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 21, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 22, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "active", to: "implemented", at: "2026-10-16T03:57:52Z", actor: "human:timo", revision: 35, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":1}}}
---
## Outcome

The deployment document may turn ingress authentication off for model calls:
`ingress_auth = "none"` serves the three wire paths without an `authorization` header, for a
gateway that sits behind something that authenticates already (a mesh, a private network, one
machine). `bearer`, the default, keeps today's behaviour. The owner-only surface does not open:
`/metrics` and `/v1/routes` still take the owner secret, and without one they are refused.

## Why

Milestone 2 of the any-wire proxy plan (operator-approved, 2026-10-16): a transparent proxy for
local clients that need not hold a gateway token. Today `owner_secret_file` is required
(`crates/llm-gateway-cli/src/config.rs:50`, `spec/domains/deployment.yaml:432`) and every request
outside the probes and public routes authenticates first (`crates/llm-gateway/src/server.rs:614-618`,
`:695-712`). llmgw's `--insecure` (capability matrix row C3) had no counterpart.

## The rule

- `ingress_auth`: `bearer` (default) or `none`; anything else is `config:schema`.
- `bearer`: `owner_secret_file` is required (`config:value` when absent), as today.
- `none`: `owner_secret_file` is optional. A request to a wire path of a gateway composed with a
  relay is admitted without authentication: no `authorization` header is read, and one sent is
  ignored (never forwarded, as before). Every other path keeps its order: the probes and public
  routes as today; `/metrics`, `/v1/routes`, `/v1/routes/{alias}` and any unknown path still
  authenticate with the owner secret. With no owner secret they are refused (`credential-absent`
  without a header, `credential-rejected` with one), so the surface still cannot be enumerated.
- `none` writes `b10x-llm-gateway: warning ingress auth is off (ingress_auth = "none"): model
  calls are served without a credential` to standard error before `listening on`.
- The gateway crate gains `IngressAuth` on `GatewayConfig` and a verifier that admits nobody;
  it still declares no dependency (`crates/llm-gateway/tests/dependency_boundary.rs`).

## ESS first

`spec/domains/deployment.yaml` (DeploymentConfiguration `ingress_auth`, `owner_secret_file`
optional, the warning line), `spec/domains/gateway.yaml` (Gateway `ingress_auth`, Exchange step 5
and Relay step 1; the `Relay` program's `ingress_auth`). Validated before code.

## Acceptance

1. With `ingress_auth = "none"`, a chat, a responses and a messages request without
   `authorization` are relayed, and one with a wrong bearer is relayed too (scenario).
2. With `none` and no owner secret, `GET /metrics` and `GET /v1/routes` are `credential-absent`
   without a header and `credential-rejected` with one; with `none` and an owner secret they are
   served to that secret only (scenario and binary test).
3. With `bearer` (and with the key absent) behaviour is unchanged: the existing credential
   scenarios pass unedited, and a `bearer` document without `owner_secret_file` is `config:value`.
4. The binary started with `none` writes the warning line before `listening on` (binary test).
5. `dependency_boundary.rs` passes unchanged.

## Scope

Confirmed at the close of wave 2026-10-16-w01 from the unit's diff (`git diff bf4158b 1f500d4`,
`adv/ingress-auth-optional` and `adv2/ingress-auth-optional`). Corrections to the list drafted
before the wave: `crates/llm-gateway/tests/gateway.rs` and `crates/llm-gateway-cli/tests/binary.rs`
were not changed (the binary is tested in `crates/llm-gateway-cli/tests/hosted.rs` instead), and
these were changed beside the list: `crates/llm-gateway/src/metrics.rs` (the usage record's doc),
`crates/llm-gateway-cli/src/lib.rs`, `crates/llm-gateway-cli/tests/adversary_w02.rs` and
`spec/domains/upstream.yaml` and `docs/llmgw-capability-matrix.md` (their citations into
`serve.rs`, which moved five lines; row C3 is now covered), the status page source and its
generated pages, and the adversary cases in `crates/llm-gateway/tests/adversary_w10_ingress*.rs`.

- `crates/llm-gateway/src/server.rs`, `crates/llm-gateway/src/auth.rs`, `crates/llm-gateway/src/lib.rs`
- `crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs`,
  `crates/llm-gateway-cli/src/relaying.rs`
- `checks/conformance/src/gateway.rs` (`ingress_auth`)
- `contracts/gateway/scenarios/ingress-*`
- `spec/domains/deployment.yaml`, `spec/domains/gateway.yaml`
- `docs/gateway.md`, `website/docs/reference/deployment-document.md`,
  `website/docs/concepts/single-owner-gateway.md`, `README.md`, `CHANGELOG.md`
- Tests: `crates/llm-gateway-cli/tests/config.rs`, `crates/llm-gateway-cli/tests/hosted.rs`

## Depends on

`story:hosted-static-headers` (and through it `story:hosted-base-path`): all three change
`crates/llm-gateway-cli/src/config.rs`, `spec/domains/deployment.yaml`,
`spec/domains/gateway.yaml`, `checks/conformance/src/gateway.rs`, `docs/gateway.md`,
`website/docs/reference/deployment-document.md` and `CHANGELOG.md`, so this story runs last
(`review-result:critic-parallel-w10-round-1`).
