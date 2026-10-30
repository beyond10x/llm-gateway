---
format: aep.planning-md/3
id: story:relayed-session-header
kind: story
status: active
title: A relayed OpenCode Go request carries its own session header
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/routes.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T04:55:18Z", actor: "human:timo", revision: 6}
- {from: "proposed", to: "active", at: "2026-10-30T04:55:18Z", actor: "human:timo", revision: 7}
---
## Outcome

A request the gateway relays to an OpenCode Go base URL (`https://opencode.ai/inference/go/…`)
carries its own `x-opencode-session`, chosen in this order:

1. the caller's own `x-opencode-session` header, passed through when the request has one (one
   printable ASCII token, at most 256 bytes; anything else is ignored, never echoed);
2. otherwise a value derived from the caller's peer address: `llmgw-` and the first 32 hex
   characters of SHA-256 over a per-process random key and the peer IP, so one pod (one Loom
   run today) keeps one session, two callers never share one, and the provider never sees the
   address;
3. a static `x-opencode-session` in the hosted endpoint's `headers` only when neither applies
   (a request with no peer address).

Out of scope: a caller run id. Loom's boundary sends no run-id header today (no `x-…run…`
header in loom's sources at 62f2e664); when it does, a follow-up story puts the run id before the
peer address, under the header name agreed with loom.

## Why

The provider refuses a request without the header (400 `MissingSessionID`). A relayed request
carries none today (`crates/llm-gateway-cli/src/routes.rs:585`, `docs/gateway.md` "The
upstream"); only the translated path gets one, minted by llm per request. The cluster workaround
puts one fixed value in the deployment document, so every caller shares one session. Found in
the G7 cluster run (2026-10-30), item from the orchestrator under the grant.

## Acceptance

Tests in the gateway's relay suites, each red on 1b33c588:

- A relayed request to an OpenCode Go base URL with a client `x-opencode-session` reaches the
  target with that value; a malformed client value (a space, a control byte, over 256 bytes) is
  ignored and never echoed.
- Two relayed requests from two peer addresses without the header reach the target with two
  different `llmgw-…` values; two from one peer address with the same value; neither contains
  the address.
- A configured static `x-opencode-session` is sent only when no peer address is known, and is
  never sent beside a passed-through or derived value.
- No `x-opencode-session` is sent to a base URL that is not OpenCode Go.
- `docs/gateway.md` "The upstream" and the deployment-document reference state the order;
  `cargo run --locked -p b10x-llm-gateway-conformance -- check` still passes 291/291.

## Open

Decided by b10x-chef-orchestrator (2026-10-30), under the grant: under `ingress_auth = "none"`
the caller is identified by its run id when the request carries one, else by the peer address.

- Run id: Loom's boundary sidecar knows the agent run and sends it; the header name is agreed
  with loom when the story is scoped.
- Peer address otherwise: Loom runs one pod per run, so the peer address is per run today; the
  run id keeps that true behind a shared proxy.
- Not per connection: clients reuse and drop connections, so one run would get several sessions.
