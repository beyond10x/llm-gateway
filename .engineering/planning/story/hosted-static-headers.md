---
format: aep.planning-md/3
id: story:hosted-static-headers
kind: story
status: implemented
title: A hosted endpoint is sent its declared static non-secret headers
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:hosted-base-path
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
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/adversary_w10_static_headers.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/adversary_w10_static_headers_pass_2.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/hosted.rs
- confidence: cited
  path: crates/llm-gateway-docs/src/status.rs website/data/status.json website/docs/status.mdx
- confidence: cited
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_w10_static_headers.rs
- confidence: cited
  path: crates/llm-gateway/tests/adversary_w10_static_headers_pass_2.rs
- confidence: cited
  path: crates/llm-gateway/tests/gateway.rs
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
revision: 27
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 17}
- {from: "proposed", to: "active", at: "2026-10-16T02:29:20Z", actor: "human:timo", revision: 18}
- {from: "active", to: "implemented", at: "2026-10-16T03:57:52Z", actor: "human:timo", revision: 27, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":1}}}
---
## Outcome

A hosted endpoint may declare a map of static, non-secret request headers (`headers`), which the
relay writes into every request head it sends that endpoint: Anthropic's `anthropic-version`,
OpenRouter's `HTTP-Referer` and `X-Title`. A header that frames or routes the request, carries a
credential, or could end its line is refused at start.

## Why

Milestone 2 of the any-wire proxy plan (operator-approved, 2026-10-16). The Anthropic API refuses
a request without `anthropic-version`; OpenRouter attributes traffic by `HTTP-Referer` and
`X-Title`. The relay builds the upstream head alone and forwards no client header
(`crates/llm-gateway/src/relay.rs:978-1003`), so today no hosted endpoint can be sent one.

## The rule

`[hosted.<alias>.headers]`, optional, a TOML table of string values:

- at most 32 entries; a name is 1..=64 RFC 9110 token characters, matched without regard to case
  and written lowercase; two names equal without regard to case are refused;
- a name is none of the headers the gateway writes or that frame a message (`host`,
  `authorization`, `content-type`, `content-length`, `connection`, `transfer-encoding`, `te`,
  `trailer`, `upgrade`, `expect`, `keep-alive`, `proxy-authorization`, `proxy-connection`), and
  not the endpoint's `credential_header`: a credential stays in `credential_file`;
- a value is 1..=1024 bytes of printable ASCII and space (0x20-0x7e), with no leading or trailing
  space, so no CR, LF or NUL;
- every violation is `config:value` naming `hosted.<alias>.headers`, and quotes no value.

The relay writes them after `host`, in name order, before the credential header and the framing
headers. No client header is forwarded, as before.

## ESS first

`spec/domains/deployment.yaml` HostedDeclaration `headers`, `spec/domains/upstream.yaml`
HostedEndpoint `headers`, `spec/domains/gateway.yaml` `Relay` step 7 and CompositionRefusal
`static-header:<rule>`; the `Relay` program's `endpoint` gains `headers`. Validated before code.

## Acceptance

1. An endpoint declaring `anthropic-version = "2023-06-01"` and `X-Title = "b10x"` receives
   `anthropic-version: 2023-06-01` and `x-title: b10x` in its head on every request, beside its
   credential, and the client's own `anthropic-version` is still not forwarded (scenario).
2. A reserved name, `authorization`, the endpoint's own `credential_header`, a name repeated
   without regard to case, more than 32 entries, an empty value, a value over 1024 bytes and a
   value holding CR, LF or a byte outside 0x20-0x7e are each refused `config:value` (tests), and
   composing such a header in the `Relay` program is `static-header:<rule>` (scenario).
3. An endpoint with no `headers` sends the head it sends today (existing hosted scenarios).

## Scope

Confirmed at the close of wave 2026-10-16-w01 from the unit's diff (`git diff 69905b5 660a165`,
`adv/hosted-static-headers` and `adv2/hosted-static-headers`): every file listed below was
changed, and so were `crates/llm-gateway-cli/src/lib.rs` (exports), `crates/llm-gateway/tests/gateway.rs`
(the `static-header-value-bytes` bound), the status page source and its generated pages, README.md,
and the adversary cases in `crates/llm-gateway/tests/adversary_w10_static_headers*.rs` and
`crates/llm-gateway-cli/tests/adversary_w10_static_headers*.rs`.

- `crates/llm-gateway/src/relay.rs`: `StaticHeader`, `RelayTarget::headers`, the request head
- `crates/llm-gateway/src/error.rs` (`StaticHeaderError`), `crates/llm-gateway/src/lib.rs`
- `crates/llm-gateway-cli/src/hosted.rs`, `crates/llm-gateway-cli/src/config.rs`
- `checks/conformance/src/gateway.rs` (`endpoint.headers`)
- `contracts/gateway/scenarios/hosted-*header*`
- `spec/domains/deployment.yaml`, `spec/domains/upstream.yaml`, `spec/domains/gateway.yaml`
- `docs/gateway.md`, `website/docs/reference/deployment-document.md`, `CHANGELOG.md`
- Tests: `crates/llm-gateway-cli/tests/config.rs`, `crates/llm-gateway-cli/tests/hosted.rs`

## Depends on

`story:hosted-base-path`: both change the request head in `relay.rs`, `HostedEndpoint` in
`hosted.rs`, `HostedDeclaration` in `config.rs` and the same spec entries.
