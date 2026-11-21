---
title: Admit service-account tokens
sidebar_position: 8
description: Let agent pods call the gateway with their own projected Kubernetes service-account token, checked by the cluster's TokenReview API, beside a backend that holds the owner secret.
lede: An agent pod proves who it is with a token the kubelet mounts for one audience; the gateway asks the API server whether the token is good, for that audience, and for a service account on its list, and writes the account into the call's usage record.
source: crates/llm-gateway-cli/src/token_review.rs, crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/tests/token_review.rs (a fake TokenReview API on loopback TLS), crates/llm-gateway/src/server.rs, docs/gateway.md (Service-account tokens, Bounds of the service-account review), spec/domains/deployment.yaml (TokenReviewDeclaration); the manifests below are examples, not checked by a test
---

# Admit service-account tokens

`ingress_auth = "service-account-token"` admits a model call in one of two ways:

- its bearer is the owner secret (a backend that holds it), compared first and never sent
  anywhere; or
- its bearer is a Kubernetes service-account token that the cluster's TokenReview API reports
  authenticated, for the audience the document names, and for a service account on the
  document's list.

This is the mode for a deployment whose agents reach the gateway through a loom boundary relay:
the relay replaces any bearer with the pod's projected token for a declared audience, so the
agent never holds the owner secret. A deployment with a backend and such agents names both:
`owner_secret_file` for the backend, `[token_review]` for the agents. Without
`owner_secret_file` only reviewed tokens are admitted, and `/metrics` and `/v1/routes` refuse
every request.

Every path but the three model calls and the token count (`/metrics`, `/v1/routes`,
`/dashboard`) takes the owner secret alone; a reviewed token opens none of them.

## The document

```toml
listen = "0.0.0.0:8080"
ingress_auth = "service-account-token"
owner_secret_file = "/etc/llm-gateway/owner-secret"

[token_review]
audience = "llm-gateway"
principals = [
  "system:serviceaccount:agents:coding-agent",
  "system:serviceaccount:agents:review-agent",
]
```

`audience` has no default: write the audience your agents' projected tokens carry. The gateway
asks the review for exactly that audience, so a token minted for the API server (the default
service-account token) or for another service is refused. `principals` names each service
account; there is no namespace wildcard, so a new account in the agents' namespace is admitted
only when someone adds its line. `api`, `ca_file` and `credential_file` default to the in-cluster
API server and the gateway pod's own mounted service-account files
([`[token_review]`](../reference/deployment-document.md#token-review)).

## The agent's token

The agent pod mounts a projected token for the gateway's audience. With a loom boundary relay the
operator renders this volume from the boundary's declared audience; set that audience to the
document's `audience`. By hand:

```yaml
volumes:
  - name: gateway-token
    projected:
      sources:
        - serviceAccountToken:
            audience: llm-gateway
            expirationSeconds: 600
            path: token
```

The kubelet rotates the token before it expires. The gateway keeps an admission for at most 60 s
and never past the token's own `exp`, so a revoked or expired token stops working within a
minute.

## The gateway's permission

The gateway calls `POST /apis/authentication.k8s.io/v1/tokenreviews` with its own service-account
token. Bind its service account to a role that grants only `create` on `tokenreviews`:

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: llm-gateway-token-review
rules:
  - apiGroups: ["authentication.k8s.io"]
    resources: ["tokenreviews"]
    verbs: ["create"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: llm-gateway-token-review
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: llm-gateway-token-review
subjects:
  - kind: ServiceAccount
    name: llm-gateway
    namespace: llm-gateway
```

The built-in `system:auth-delegator` ClusterRole also works, but grants more: `create` on
`subjectaccessreviews` as well, so the gateway could ask the API server whether any user may do
anything. The gateway never asks that; prefer the role above.

The gateway reads `ca_file` once, at start: after a cluster CA rotation, restart it.
`credential_file` is read again at every review, so the kubelet's rotation of the gateway's own
token needs nothing.

The gateway pod needs its service-account token mounted (`automountServiceAccountToken` left on,
or a projected token for the API server's audience at `credential_file`).

## The network

The gateway listens on plain HTTP, and the token crosses the pod network as a bearer. Whoever can
read the traffic between an agent and the gateway can replay that agent's token against the
gateway until the token expires. A loom boundary relay runs beside the agent and talks to it over
loopback; the hop from the agent's pod to the gateway is inside the cluster. Admit only that hop
with a NetworkPolicy on the gateway's namespace, and use a mesh with mutual TLS where the pod
network is not trusted:

```yaml
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: llm-gateway-from-agents
  namespace: llm-gateway
spec:
  podSelector: {}
  policyTypes: ["Ingress"]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: agents
      ports:
        - port: 8080
```

The gateway also needs egress to the API server, if an egress policy applies to its namespace.

The NetworkPolicy is also what keeps the review itself available. Before a review the gateway
reads the token's own claims, unverified, and refuses one that is not a JWT, has expired or does
not name the audience, without asking the API server. A caller that knows the audience can still
send distinct tokens naming it: each costs an API server call, and while the gateway's 8 review
slots are taken a listed agent whose admission is not cached gets `503
credential-review-unavailable`. Only callers the NetworkPolicy admits can do that.

## What a caller sees

| Case | Answer |
| --- | --- |
| The owner secret | relayed |
| A token the review admits | relayed; the usage record names `principal` |
| Another audience, a principal off the list, expired, not a token, an answer with `status.error` | `401 credential-rejected` |
| No answer from the API server: unreachable, a timeout, a non-2xx status, a malformed body, too many reviews at once | `503 credential-review-unavailable` |
| No `authorization` | `401 credential-absent` |

The principal comes from the review alone: no request header can name it. The usage event on
standard error carries `principal="system:serviceaccount:agents:coding-agent"`; no log line,
error or metric label carries the token or the review's body.
