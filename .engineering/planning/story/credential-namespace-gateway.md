---
format: aep.planning-md/3
id: story:credential-namespace-gateway
kind: story
status: active
title: A guide and manifests run the gateway in a credential namespace with ingress none and an ingress NetworkPolicy
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:kubernetes-deployment-guide
- depends_on: story:ingress-auth-optional
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: crates/llm-gateway-cli/tests/credential_namespace.rs
- confidence: cited
  path: deploy/kubernetes/credential-namespace
- confidence: cited
  path: website/docs/guides/run-in-a-credential-namespace.md
- confidence: cited
  path: website/docs/index.md
- confidence: cited
  path: website/sidebars.ts
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T00:48:59Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T00:48:59Z", actor: "human:timo", revision: 4}
---
## Outcome

The repository holds the manifests and a guide for a gateway that runs in a namespace of its own,
`agent-credentials`, so that agent pods in the namespace `agents` call a model without holding the
provider key. Model calls take no credential (`ingress_auth = "none"`); the only door is a
NetworkPolicy that admits ingress from the namespace `agents` on port 8080.

## Acceptance

1. `deploy/kubernetes/credential-namespace/` holds `00-namespace.yaml`, `10-network-policy.yaml`
   and `20-gateway.yaml` (ConfigMap, Secret, Deployment, Service), all in the namespace
   `agent-credentials`, with no comment in any file.
2. The gateway document listens on `0.0.0.0:8080`, sets `ingress_auth = "none"` and names the
   provider key as `credential_file` of a hosted model; the Deployment passes the key only as a
   mounted file (no `env`, no `envFrom`), copied by the init container so the trusted-file reader
   accepts it, and mounts no service-account token.
3. The NetworkPolicy selects the gateway's pods, has policy type `Ingress` only, and one rule whose
   one peer is a namespace selector for `agents` and whose one port is TCP 8080.
4. The guide `website/docs/guides/run-in-a-credential-namespace.md` shows each file exactly as it is
   in the repository, pastes the start lines the binary prints on the ConfigMap's document, and
   says that the NetworkPolicy is the only access control and must be confirmed to be enforced.
5. `crates/llm-gateway-cli/tests/credential_namespace.rs` checks 1 to 4 and starts the binary on
   the ConfigMap's document after running the init script, and `task check` stays green.

## Not in scope

A caller-identity check at the gateway (a service-account token verifier behind the owner
verifier): it folds into `story:per-run-client-tokens`. A cluster run; the manifests are checked
against the binary and the schema of their own text, not applied.
