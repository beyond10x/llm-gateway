---
format: aep.planning-md/3
id: story:subscription-passthrough
kind: story
status: draft
title: A hosted route to api.anthropic.com forwards the client's own credential
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: cited
  path: docs/gateway.md
revision: 1
---
## Outcome

`auth_kind = "client"` on a hosted route, allowed only for `https://api.anthropic.com`, forwards
the client's `authorization`, `x-api-key`, `anthropic-version` and `anthropic-beta` unchanged and
never logs them, so Claude Code on a Claude subscription works through a local gateway. A route
may serve every model name with a prefix (`model_prefix = "claude-"`), relayed with the name as
the client sent it.

## Acceptance

1. A loopback fixture receives the four headers byte for byte; no other client header, and none
   of them on any other route.
2. `auth_kind = "client"` with another host, without `ingress_auth = "none"`, or with a
   credential is `config:value`.
3. `docs/gateway.md` states the exception to "no header of the client's request is forwarded".
