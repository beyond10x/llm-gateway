---
format: aep.planning-md/3
id: story:model-tool-calling
kind: story
status: implemented
title: A model declares tool calling, and a tool request to a model without it is refused before a pod starts
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:provider-key-files
- depends_on: story:cold-start-hold
- depends_on: story:gateway-observability
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/relaying.rs
- confidence: inferred
  path: crates/llm-gateway/src/body.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/clients.yaml
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T18:12:15Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-08T18:12:15Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-08T19:44:56Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"test_result":1,"review_outcome":5,"verification":1}}}
---
## Outcome

A model in the deployment document declares whether its vLLM server parses tool calls. A relayed
request whose body carries a non-empty top-level `tools` array, for a model that declares it does
not, is refused `tools-not-served` (400) before any target is asked for, so it never starts a pod.

## Why

Claude Code and Codex send tools on every request (`docs/design/runpod-clients.md` § 1, § 2). A
pod started without `--enable-auto-tool-choice` and a `--tool-call-parser` cannot answer them, and
without this refusal the gateway would start and bill a pod to fail the request.

## ESS first

`llm-gateway.clients.ToolCalling` and `llm-gateway.clients.ClientRefusal` are declared `PLANNED` in
`spec/domains/clients.yaml`. This story moves `ToolsNotServed` into `llm-gateway.gateway.Refusal`,
adds the per-model declaration to `llm-gateway.deployment.ModelDeclaration`, and settles the
`UNMAPPED` marker on `ToolCalling` (its own key, or inferred from `extra_vllm_args`) as `DECIDED`,
before any code.

## Acceptance

1. `ess specify validate --path spec` passes with `ToolsNotServed` in `llm-gateway.gateway.Refusal`
   and the tool-calling declaration on `ModelDeclaration`; the `UNMAPPED` marker on `ToolCalling`
   is replaced by a `DECIDED` line.
2. A `Relay` conformance scenario sends a request with a non-empty `tools` array for a model
   declaring `Absent`: the answer is 400 `tools-not-served`, and the observation records zero
   target acquisitions.
3. Three `Relay` scenarios, one each, are relayed unchanged to the fixture pod: a request with a
   non-empty `tools` array for a model declaring `Parsed`; a request with no `tools` key for a model
   declaring `Absent`; a request with `"tools": []` for a model declaring `Absent`.
4. `docs/gateway.md` lists `tools-not-served` in its refusal table with status 400, and the
   crate's test that provokes every `RefusalCode` provokes it.
5. A process test starts the binary with a document declaring one model `Parsed` and one `Absent`,
   through the test seam `story:gateway-deployment` adds: a tool request for the `Absent` model is
   answered `tools-not-served` with the fixture pod receiving nothing, and the same request for
   the `Parsed` model reaches the fixture pod.
6. A document that omits the declaration still parses and the model takes `Absent`, so an omitted
   key never starts a pod for a tool request. `spec/domains/deployment.yaml` records that default
   as `DECIDED`, and a test in `crates/llm-gateway-cli/tests/config.rs` pins it.

## Depends on

- `story:provider-key-files`: both change the deployment document in
  `crates/llm-gateway-cli/src/config.rs` and `spec/domains/deployment.yaml`.
- `story:cold-start-hold`, and through it `story:gateway-deployment`: all three change the relay
  path in `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway-cli/src/serve.rs` and
  `crates/llm-gateway/tests/wire_relay.rs`, and criterion 5 uses the binary's relay seam.
- `story:gateway-observability`: both change `crates/llm-gateway/src/relay.rs`,
  `checks/conformance/src/gateway.rs`, `docs/gateway.md`, `spec/domains/deployment.yaml` and
  `spec/domains/gateway.yaml`.
- `story:live-runpod-wiring`: both change `spec/domains/deployment.yaml` and
  `crates/llm-gateway-cli/src/serve.rs`.

## Neighbours

Ordered after this story through edges: `story:pod-proxy-tls` and `story:public-model-listing`
(`spec/domains/deployment.yaml`, `docs/gateway.md`, `crates/llm-gateway-cli/src/serve.rs`), and,
through `story:client-qualification`, `story:hosted-endpoints`, `story:target-fallback` and
`story:gateway-translation` (`spec/domains/deployment.yaml`, `spec/domains/gateway.yaml`,
`docs/gateway.md`, the `RefusalCode` set in `crates/llm-gateway/src/error.rs`).

## Order changed 2026-10-08

`depends_on story:live-runpod-wiring` was taken back: it ordered two stories that both change
`spec/domains/deployment.yaml` and `crates/llm-gateway-cli/src/serve.rs`. The order is reversed:
`story:live-runpod-wiring` now depends on this story. Criterion 5 uses the test seam
`story:gateway-deployment` added, which exists on `main`.

## Scope as built (wave 2026-10-08-w05)

Confirmed by the implementor: the binary-side change landed in `crates/llm-gateway-cli/src/relaying.rs`
(`start_relaying`), not `serve.rs`, which is untouched; also `crates/llm-gateway/src/lib.rs` (export) and
`crates/llm-gateway-cli/tests/relaying.rs` (criterion 5). Every enumerated deployment-document value is now read
from a TOML string only (`from_string_only!` in `config.rs`), after adversary pass 1.
