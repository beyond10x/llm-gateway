---
format: aep.planning-md/3
id: story:runpod-rest-host
kind: story
status: draft
title: The Runpod REST client calls a host that resolves, and says which one it could not reach
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The gateway's Runpod REST client calls a host that resolves and answers, and a wake of a model whose
provider it cannot reach answers `target-unavailable` naming the provider.

## Why

Measured on the owner's host, 2026-11-26, with 0.25.0 serving a managed model on `[providers.runpod]`
with a keychain `api_key`: the binary started, logged `Runpod REST API unreachable; its models
answer target-unavailable provider="runpod"` and `runpod account read failed`, and
`getent hosts rest.runpod.io` returned no address while `api.runpod.io` resolved. The client's base
`https://rest.runpod.io/v1` is compiled into the binary (deployment document, `api_key`). 0.24.0
logged the same account read failure on the same host before the switch. No pod was started.

## Acceptance

1. The pull request names which host the Runpod REST client uses, from Runpod's own documentation
   or a measured answer (status line only, no key, no pod), and the binary uses it.
2. A test against a fake Runpod whose name does not resolve: a wake of a managed model answers
   `target-unavailable`, its message naming the provider, and starts nothing.
3. The startup warning names the host it could not reach.
