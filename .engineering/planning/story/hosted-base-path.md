---
format: aep.planning-md/3
id: story:hosted-base-path
kind: story
status: active
title: A hosted endpoint's base_url may carry a path, joined to the wire path
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/hosted.rs
- confidence: cited
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/upstream.yaml
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 19
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 18, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 19, decided_on: {"recorded":{"review_outcome":1}}}
---
## Outcome

A hosted endpoint's `base_url` may carry a path, so a vendor whose API lives below a prefix
(`https://openrouter.ai/api/v1`) is reachable without a proxy in front of it. The upstream path is
the base path joined to the wire path, the `host` header stays the endpoint's authority, and a
base URL that could address anything but a path below that prefix is refused at start.

## Why

Milestone 2 of the any-wire proxy plan (operator-approved, 2026-10-16): the gateway reaches
RunPod/vLLM, OpenRouter, the Anthropic API and the OpenAI API directly. Today `base_url` must be a
scheme and an authority only (`crates/llm-gateway-cli/src/hosted.rs:116-143`) and the upstream
path always equals the ingress path (`crates/llm-gateway/src/relay.rs:926`), so OpenRouter's
`/api/v1/chat/completions` cannot be reached.

## The base-path rule

Every wire path starts `/v1/` (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`, and the
output clamp's `/v1/messages/count_tokens`).

- A `base_url` with no path, or only `/`, keeps today's behaviour: the upstream path is the wire
  path (`https://api.anthropic.com` + messages = `/v1/messages`).
- A `base_url` with a path `B` (a trailing `/` dropped) replaces the wire path's leading `/v1`:
  the upstream path is `B` followed by the rest of the wire path.
  `https://openrouter.ai/api/v1` + chat = `/api/v1/chat/completions`;
  `https://api.openai.com/v1` + responses = `/v1/responses`;
  `https://gw.example/openai` + chat = `/openai/chat/completions`.
- `B` is 1..=32 segments of 1+ characters of `A-Z a-z 0-9 - . _ ~`, at most 256 bytes, with no
  segment `.` or `..` and no empty segment. A query, a fragment, user information, `%`, `\` or
  any other byte is `config:value`, as today.
- `host` is the endpoint's authority, unchanged. A Runpod pod has no base path.

## ESS first

`spec/domains/deployment.yaml` HostedDeclaration `base_url` and `spec/domains/upstream.yaml`
HostedEndpoint `base_url` state the rule; `spec/domains/gateway.yaml` `Relay` step 7 says the
target is asked on its base path joined to the wire path; the `Relay` program's `endpoint` gains
`base_path`. Validated with `ess specify validate --path spec` before code.

## Acceptance

1. A hosted model whose `base_url` is `http://<fixture>/api/v1` is relayed on chat, responses and
   messages to `/api/v1/chat/completions`, `/api/v1/responses` and `/api/v1/messages`, and its
   output-clamp count to `/api/v1/messages/count_tokens` (conformance scenario and a CLI test).
2. A `base_url` without a path, or with only `/`, reaches the same paths as before this story
   (the existing hosted scenarios pass unedited, plus a unit test). A `base_url` ending `/v1`,
   refused `config:value` before this story, now reaches `/v1/chat/completions` (a unit test).
3. `host` is the authority alone for a base URL with a path (a CLI test reading the head).
4. A base URL with a `..` or `.` segment, an empty segment, a query, a fragment, user information
   or a `%` is refused `config:value` naming `hosted.<alias>.base_url` (tests), and composes
   nothing in the `Relay` program (`endpoint:base-url`, a scenario).

## Scope

- `crates/llm-gateway/src/relay.rs`: `BasePath`, `RelayTarget::base_path`, the request head's path
- `crates/llm-gateway/src/error.rs`, `crates/llm-gateway/src/lib.rs`
- `crates/llm-gateway-cli/src/hosted.rs`, `crates/llm-gateway-cli/src/config.rs`
- `checks/conformance/src/gateway.rs` (`endpoint.base_path`)
- `contracts/gateway/scenarios/hosted-*base*`
- `spec/domains/deployment.yaml`, `spec/domains/upstream.yaml`, `spec/domains/gateway.yaml`
- `docs/gateway.md`, `website/docs/reference/deployment-document.md`, `CHANGELOG.md`
- Tests: `crates/llm-gateway-cli/tests/hosted.rs`, `crates/llm-gateway-cli/tests/config.rs`

Shares `relay.rs` request head, `hosted.rs`, `config.rs` and the three spec domains with
`story:hosted-static-headers` and `story:ingress-auth-optional`: the three run one after another.

## Not in scope

Protocol translation and loading routes from llm's `llm.routes/1` (Milestone 2 part B,
`story:gateway-translation`).
