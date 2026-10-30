---
format: aep.planning-md/3
id: approval-record:d243-g1-credential-namespace
kind: approval-record
status: active
title: Credential-namespace gateway manifests approved by the orchestrator with decision E1
relations:
- decides: story:credential-namespace-gateway
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-30T00:49:00Z", actor: "human:timo", revision: 2}
---
## Stop

Approved by `b10x-chef-orchestrator` on 2026-10-30 under the grant in the workspace instructions:
"do item D243-G1 ... A Kubernetes guide and manifest for a credential-namespace gateway with
ingress none and an ingress NetworkPolicy".

## Reason

The orchestrator's decision E1 applies: "for the first runs the gateway uses `ingress_auth = "none"`
plus an ingress NetworkPolicy that admits only namespace `agents`". The service-account verifier
is not started here; it folds into `story:per-run-client-tokens` later.

## Limits

Docs and manifests only. One pull request through the bot App, merged. No cluster is touched; the
manifests are checked by `crates/llm-gateway-cli/tests/credential_namespace.rs`.
