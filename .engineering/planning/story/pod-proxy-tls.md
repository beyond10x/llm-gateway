---
format: aep.planning-md/3
id: story:pod-proxy-tls
kind: story
status: draft
title: The binary reaches a pod's https proxy URL over verified TLS
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
- depends_on: story:live-runpod-wiring
- depends_on: story:model-tool-calling
- depends_on: story:public-model-listing
scope:
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/tls.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway/tests/dependency_boundary.rs
- confidence: inferred
  path: docs/hosting.md
- confidence: cited
  path: spec/domains/deployment.yaml
revision: 6
---
## Outcome

The binary's Runpod relay target reaches a pod at its `https` proxy URL over TLS, with the
certificate verified against the platform's trust roots, before any request byte is written.

## Why

`RelayTarget::connect` leaves the transport, TLS included, to the embedding (`docs/gateway.md` "The
relay"), and a Runpod pod is served at `https://<pod>-8000.proxy.runpod.net/v1/`, the address llm 0.5.0's Runpod
description gives (`llm_providers::descriptions::runpod()`, used by `crates/llm-runpod/src/provider.rs`; `docs/hosting.md`, the endpoint row). `story:gateway-deployment` proves
the relay against a loopback pod, and no story gives the binary a TLS client
(`docs/design/runpod-clients.md`, "What exists and what is missing"). `story:hosted-endpoints`
reuses the client and the test-root seam this story adds.

## ESS first

`spec/domains/deployment.yaml` declares that a pod endpoint is reached over TLS with a verified
certificate, and which trust roots the shipped binary uses, before any code.

## Acceptance

1. A process test relays a chat request to a loopback `https` fixture pod whose certificate chains
   to a test root handed in through a test seam in `crates/llm-gateway-cli`: the fixture receives
   the request and the client receives its answer.
2. A fixture whose certificate the binary does not trust ends the request `upstream-failed`, and
   the fixture records zero request bytes.
3. The shipped binary has no document key and no option that adds a trust root or disables
   verification; a test checks the document refuses such a key as `config:schema`.
4. `crates/llm-gateway/tests/dependency_boundary.rs` passes unchanged: no TLS crate enters the
   gateway crate's closure.
5. `spec/domains/deployment.yaml` carries a `DECIDED` line naming the verified-certificate rule and
   the trust roots of the shipped binary, and `ess specify validate --path spec` passes.

## Depends on

- `story:gateway-deployment`, through `story:live-runpod-wiring`: the pool-backed relay target this
  story gives TLS.
- `story:live-runpod-wiring`: both compose the binary's Runpod pieces in
  `crates/llm-gateway-cli/Cargo.toml`, `src/serve.rs`, `tests/binary.rs` and
  `spec/domains/deployment.yaml`.
- `story:model-tool-calling`, and through it `story:gateway-observability`: both change
  `spec/domains/deployment.yaml`, `crates/llm-gateway-cli/src/serve.rs`,
  `crates/llm-gateway-cli/Cargo.toml`, `tests/binary.rs` and `tests/config.rs`.
- `story:public-model-listing`: both change `crates/llm-gateway-cli/src/serve.rs`.
