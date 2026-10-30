---
format: aep.planning-md/3
id: story:relayed-session-header
kind: story
status: draft
title: A relayed OpenCode Go request carries its own session header
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 2
---
## Outcome

A request the gateway relays to an OpenCode Go base URL (`https://opencode.ai/inference/go/…`)
carries its own `x-opencode-session`:

1. the caller's own `x-opencode-session` header, passed through when the request has one;
2. otherwise a value the gateway derives per caller or per run, so two callers never share one
   session and one caller's turns keep theirs;
3. a static `x-opencode-session` in the hosted endpoint's `headers` only as the fallback when
   neither applies.

## Why

The provider refuses a request without the header (400 `MissingSessionID`). A relayed request
carries none today (`crates/llm-gateway-cli/src/routes.rs:585`, `docs/gateway.md` "The
upstream"); only the translated path gets one, minted by llm per request. The cluster workaround
puts one fixed value in the deployment document, so every caller shares one session. Found in
the G7 cluster run (2026-10-30), item from the orchestrator under the grant.

## Acceptance

- A relayed request to an OpenCode Go base URL with a client `x-opencode-session` reaches the
  target with that value.
- Two relayed requests from two callers without the header reach the target with two different
  values; two from one caller (or one run) with the same value.
- A configured static `x-opencode-session` is sent only when neither of the above produced one.
- The header is never sent to a base URL that is not OpenCode Go.
- `docs/gateway.md` "The upstream" and the deployment-document reference state the order.

## Open

Decided by b10x-chef-orchestrator (2026-10-30), under the grant: under `ingress_auth = "none"`
the caller is identified by its run id when the request carries one, else by the peer address.

- Run id: Loom's boundary sidecar knows the agent run and sends it; the header name is agreed
  with loom when the story is scoped.
- Peer address otherwise: Loom runs one pod per run, so the peer address is per run today; the
  run id keeps that true behind a shared proxy.
- Not per connection: clients reuse and drop connections, so one run would get several sessions.
