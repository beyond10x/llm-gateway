---
format: aep.planning-md/3
id: story:listener-closes-on-sigterm
kind: story
status: draft
title: The listener closes when the drain starts
relations:
- decomposes: epic:gateway
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/adversary_k8s_guide.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: website/docs/guides/run-on-kubernetes.md
revision: 7
---
## Outcome

On `SIGTERM` the gateway closes its listener when the drain starts: a connection made after the
signal is refused at once, while every connection already accepted is still answered before the
gateway exits with status 0.

## Why

Today the listener stays open until the last accepted connection is answered, so a client that
Kubernetes still routes to a stopping pod completes its handshake and waits unanswered until the
drain ends. Kubernetes expects a stopping container to refuse new connections, so the client
retries on another endpoint. The Kubernetes guide states today's behaviour; this story changes it.

## Acceptance

`a_connection_made_after_sigterm_is_refused_while_an_accepted_one_drains` in
`crates/llm-gateway-cli/tests/adversary_k8s_guide.rs` runs (its `#[ignore]` removed) and passes:
after `SIGTERM`, a new connection is refused within 2 s while an accepted one is still answered.
The drain is modelled in the specification first (`spec/domains/deployment.yaml`) with a
conformance scenario for a connection made during the drain. The guide's "Stop it" section
states the new behaviour.

## Scope (inferred)

`crates/llm-gateway/src/server.rs` (listener, `begin_drain`, `shutdown`),
`crates/llm-gateway-cli/src/` (signal handling), `spec/domains/deployment.yaml`,
`contracts/`, `crates/llm-gateway-cli/tests/adversary_k8s_guide.rs`,
`website/docs/guides/run-on-kubernetes.md`, `CHANGELOG.md`.
