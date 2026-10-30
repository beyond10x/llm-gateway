---
title: Run in a credential namespace
sidebar_position: 7
description: Run the gateway in its own Kubernetes namespace so agent pods call a model without holding its provider key, with model calls taking no credential and a NetworkPolicy admitting only the agents' namespace.
lede: The provider key lives in a namespace of its own, read from a mounted file by a gateway that asks its callers for nothing; the network is the only door, and a NetworkPolicy lets one namespace through it.
source: deploy/kubernetes/credential-namespace/*.yaml (the manifests below are those files), crates/llm-gateway-cli/tests/credential_namespace.rs (starts the binary on the page's document and checks every manifest), crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/trusted.rs; the start lines below are pasted from a run of the binary
---

# Run in a credential namespace

An agent pod that calls a model needs a way to reach it, and nothing in the pod needs the key
that pays for it. This page runs the gateway in a namespace of its own, `agent-credentials`, with
the provider key as a mounted file there, and lets pods of the namespace `agents` reach it:

```text
namespace agents                         namespace agent-credentials
agent pod  ── POST /v1/messages ──►  Service llm-gateway :8080  ──►  gateway pod
(no provider key, no gateway token)   NetworkPolicy: from `agents`    provider key file, mode 0400
```

It builds on [Run on Kubernetes](run-on-kubernetes.md): the file rules, the init container that
copies a Secret file into memory with the right owner and mode, and the stop are the same, and
that page explains them. This page changes four things.

- The key is the model's provider key, read from `credential_file` of a `[hosted.<alias>]` model.
  The gateway reads it through the same trusted-file reader as every secret: a Secret volume
  mounted straight into the gateway is refused when the pod has an `fsGroup`, so the init container
  copies it. The gateway reads a credential only from a file: a key in an environment variable
  has no place in the deployment document, and a routes document that names an `env:` secret is
  refused at start.
- `ingress_auth = "none"`: a model call carries no credential, so no owner secret exists and the
  caller holds no token. [Model calls without a credential](../concepts/single-owner-gateway.md#model-calls-without-a-credential)
  states exactly what that turns off.
- `listen = "0.0.0.0:8080"`, because the pod's address is not loopback.
- A NetworkPolicy admits ingress from the namespace `agents` on `8080` and from nowhere else.

## The manifests

Apply them in the order of their file names: the namespace, the policy and then the gateway, so
that the gateway never runs in a namespace whose policy is not yet there. The files are in
[`deploy/kubernetes/credential-namespace/`](https://github.com/beyond10x/llm-gateway/tree/main/deploy/kubernetes/credential-namespace).

```yaml title="00-namespace.yaml"
apiVersion: v1
kind: Namespace
metadata:
  name: agent-credentials
  labels:
    pod-security.kubernetes.io/enforce: baseline
```

The namespace enforces `baseline`, not `restricted`: the init container runs as root with only
the `CHOWN` capability, which `baseline` admits and `restricted` refuses.

```yaml title="10-network-policy.yaml"
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: llm-gateway
  namespace: agent-credentials
spec:
  podSelector:
    matchLabels:
      app: llm-gateway
  policyTypes: ["Ingress"]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: agents
      ports:
        - protocol: TCP
          port: 8080
```

A pod that a NetworkPolicy selects accepts only what one of its rules names, so the single rule
is the whole list: every pod of the namespace `agents` on `8080`, and no other pod, namespace or
address, and no other port. The policy sets no egress rule: the gateway has to reach the provider.

```yaml title="20-gateway.yaml"
apiVersion: v1
kind: ConfigMap
metadata:
  name: llm-gateway
  namespace: agent-credentials
data:
  gateway.toml: |
    listen = "0.0.0.0:8080"
    ingress_auth = "none"

    [hosted.claude]
    wires = ["messages"]
    base_url = "https://api.anthropic.com"
    auth_kind = "api-key"
    credential_file = "/run/llm-gateway/provider-key"
    credential_header = "x-api-key"
    upstream_model = "replace-with-the-provider-model-id"

    [hosted.claude.headers]
    anthropic-version = "2023-06-01"
---
apiVersion: v1
kind: Secret
metadata:
  name: llm-gateway-secrets
  namespace: agent-credentials
type: Opaque
stringData:
  provider-key: "replace-with-the-provider-api-key"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: llm-gateway
  namespace: agent-credentials
spec:
  replicas: 1
  selector:
    matchLabels:
      app: llm-gateway
  template:
    metadata:
      labels:
        app: llm-gateway
    spec:
      automountServiceAccountToken: false
      terminationGracePeriodSeconds: 330
      initContainers:
        - name: copy-secrets
          image: busybox:1.37
          command:
            - sh
            - -c
            - |
              set -e
              for file in provider-key; do
                rm -f "/run/llm-gateway/$file"
                cp "/in/$file" "/run/llm-gateway/$file"
                chmod 0400 "/run/llm-gateway/$file"
                chown 65532:65532 "/run/llm-gateway/$file"
              done
          securityContext:
            runAsUser: 0
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
              add: ["CHOWN"]
          volumeMounts:
            - name: secrets-in
              mountPath: /in/provider-key
              subPath: provider-key
              readOnly: true
            - name: secrets
              mountPath: /run/llm-gateway
      containers:
        - name: gateway
          image: ghcr.io/beyond10x/llm-gateway:<version>
          args: ["--config", "/etc/llm-gateway/gateway.toml"]
          ports:
            - name: http
              containerPort: 8080
          securityContext:
            runAsNonRoot: true
            runAsUser: 65532
            runAsGroup: 65532
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
          readinessProbe:
            httpGet:
              path: /ready
              port: http
            periodSeconds: 5
          livenessProbe:
            httpGet:
              path: /health
              port: http
            periodSeconds: 10
          lifecycle:
            preStop:
              sleep:
                seconds: 10
          volumeMounts:
            - name: config
              mountPath: /etc/llm-gateway/gateway.toml
              subPath: gateway.toml
              readOnly: true
            - name: secrets
              mountPath: /run/llm-gateway
              readOnly: true
      volumes:
        - name: config
          configMap:
            name: llm-gateway
        - name: secrets-in
          secret:
            secretName: llm-gateway-secrets
            defaultMode: 0400
        - name: secrets
          emptyDir:
            medium: Memory
---
apiVersion: v1
kind: Service
metadata:
  name: llm-gateway
  namespace: agent-credentials
spec:
  type: ClusterIP
  selector:
    app: llm-gateway
  ports:
    - name: http
      port: 8080
      targetPort: http
      protocol: TCP
```

Write the release you run for `<version>`, the provider's model id for `upstream_model`, and
change the `[hosted.claude]` table to the provider you use
([the hosted-endpoint keys](../reference/deployment-document.md#hosted-models)). Do not apply the
Secret with its placeholder: create the real one with
`kubectl --namespace agent-credentials create secret generic llm-gateway-secrets --from-file=provider-key`
and leave it out of version control, or write it through your secret manager. The pod mounts only
the file it names, and `automountServiceAccountToken: false` leaves it no cluster token.

```bash
kubectl apply -f 00-namespace.yaml -f 10-network-policy.yaml -f 20-gateway.yaml
```

The gateway started on the page's document, with the listen port moved to a free one for the run,
says what it is serving:

```text
b10x-llm-gateway: warning ingress auth is off (ingress_auth = "none"): model calls are served without a credential
b10x-llm-gateway: listening on 0.0.0.0:8080
```

## Call it from an agent pod

The Service's name inside the cluster is `llm-gateway.agent-credentials.svc`. An agent pod of the
namespace `agents` sends a Messages request there with no key and no `authorization` header, and
names the model by its alias:

```text
POST http://llm-gateway.agent-credentials.svc:8080/v1/messages
```

```json
{"model": "claude", "max_tokens": 64, "messages": [{"role": "user", "content": "Say hello."}]}
```

A header the caller does send is read by nothing and never forwarded; the gateway adds the
provider key alone.

## What this protects, and what it does not

- **The provider key never leaves the namespace.** It is in the Secret and in the gateway's memory
  and a memory-backed file. An agent pod can spend the key's budget through the gateway; it can
  neither read the key nor use it anywhere else.
- **The NetworkPolicy is the only access control.** With `ingress_auth = "none"` the gateway asks
  nothing of a caller that reaches port `8080`, so every pod of the namespace `agents` can make
  model calls, including a model session inside an agent pod, and two agents in that namespace
  cannot be told apart. A cluster whose network plugin does not enforce NetworkPolicy leaves the
  port open to every pod in the cluster, and the Kubernetes API accepts the policy all the same.
  Confirm that the plugin enforces it before you rely on this page: from a pod outside `agents`,
  a request to the Service must time out.
- **Only model calls are open.** `/metrics`, `/v1/routes` and every unknown path still ask for
  the owner secret, and this document names none, so they refuse every request. The probes and
  the public model list answer without one.
- **There is no per-caller token.** A gateway that checks who is calling, so that a token is
  scoped to one run or one pod, is not part of this page.
