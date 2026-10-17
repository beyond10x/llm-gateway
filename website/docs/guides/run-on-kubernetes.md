---
title: Run on Kubernetes
sidebar_position: 5
description: Run the released image on Kubernetes with the gateway's file rules unchanged, let Prometheus scrape it on its own port, and stop it without cutting an answer short.
lede: The gateway reads every secret through a trusted-file reader that refuses a file any group or other user can touch; this page mounts the files so it accepts them, copies them when the pod's fsGroup would break that, and stops the pod so it answers what it accepted.
source: Dockerfile, crates/llm-gateway-cli/src/trusted.rs, crates/llm-gateway-cli/src/serve.rs, crates/llm-gateway-cli/tests/binary.rs (the refusal pasted below is that test's expected line), docs/gateway.md (the stop); the refusal and the start lines below are pasted from runs of the binary
---

# Run on Kubernetes

The image is `ghcr.io/beyond10x/llm-gateway:<version>`; write the release you run for
`<version>`. It is built from a distroless base, contains the one binary, listens on `8080` and
runs as the user `nonroot` (uid and gid `65532`). It has **no shell**: nothing in it can run
`sh -c`, so a hook or a probe that needs a command must use something Kubernetes does itself.

The gateway's file rules do not change on a cluster. A secret file (the owner secret, a model's
`vllm_api_key_file`, a hosted endpoint's `credential_file`) must be a regular file owned by the
user the gateway runs as, or by root, with no group or other permission bit set
([the deployment document](../reference/deployment-document.md)). Everything below follows from
that rule.

## The manifests

A ConfigMap holds the deployment document, a Secret holds the owner secret, and a Deployment runs
the gateway. The document listens on `0.0.0.0:8080`, because the pod's address is not loopback,
and names the secret by the path the pod will have.

```yaml title="gateway.yaml"
apiVersion: v1
kind: ConfigMap
metadata:
  name: llm-gateway
data:
  gateway.toml: |
    listen = "0.0.0.0:8080"
    owner_secret_file = "/run/llm-gateway/owner-secret"
    metrics_listen = "0.0.0.0:9090"

    [providers.runpod]
    kind = "runpod-vllm"

    [models.small]
    provider = "runpod"
    wires = ["chat", "responses", "messages"]
    tool_calling = "parsed"
    context_window = 65536
    hf_model = "example/small-model"
    image = "vllm/vllm-openai:v0.27.1"
    gpu_types = ["NVIDIA L40S"]
    max_model_len = 65536
---
apiVersion: v1
kind: Secret
metadata:
  name: llm-gateway-secrets
type: Opaque
stringData:
  owner-secret: "replace-with-32-to-4096-printable-bytes-without-spaces"
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: llm-gateway
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
      terminationGracePeriodSeconds: 330
      initContainers:
        - name: copy-secrets
          image: busybox:1.37
          command:
            - sh
            - -c
            - |
              set -e
              for file in owner-secret; do
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
              mountPath: /in/owner-secret
              subPath: owner-secret
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
            - name: metrics
              containerPort: 9090
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
```

`defaultMode: 0400` is `256` in a manifest that is JSON. The Secret volume is mounted file by
file with `subPath`, so the pod sees only the files it names, and each file is `0400`.

A Secret volume's files are owned by **root**, and Kubernetes has no field that gives them
another owner. The gateway accepts a root-owned file, but a container that runs as `nonroot`
cannot read a root-owned `0400` file. That is why the Deployment does not mount the Secret into
the gateway container: the init container, which runs as root with only the `CHOWN` capability,
copies each file into an `emptyDir` and makes `nonroot` its owner with mode `0400`. The gateway
then reads files that are its own and private, and the rule is met without a relaxation. To
serve another secret file, add its name to the `for` list and name the copied path in the
document (`vllm_api_key_file`, `credential_file`), and mount its Secret key the same way. The
`emptyDir` is in memory, so no key reaches the node's disk. The `rm -f` before the `cp`
lets the container run again over the `emptyDir`, which lives as long as the pod: the `0400`
copy of the first run is not writable. The init container runs as uid 0 and adds `CHOWN`, which
the `restricted` Pod Security level refuses, so the namespace must enforce `baseline` or
looser.

## When the pod has an `fsGroup`

A pod that sets `securityContext.fsGroup` (a cluster policy may force one) makes Kubernetes
change every volume it mounts: it sets the group on each file and **adds the group read bit**,
so a `0400` Secret file is `0440` in the container. Mounted directly into the gateway at
`/run/llm-gateway/owner-secret`, such a file is refused at start and the pod restarts in a loop:

```bash
b10x-llm-gateway --config /etc/llm-gateway/gateway.toml
```

```text
b10x-llm-gateway: refused owner-secret:unsafe-mode: /run/llm-gateway/owner-secret: has mode 440; none of 077 may be set
```

The reader is not relaxed for this. The refusal is the gateway doing what it is for: a group
that can read the owner secret is a group that can use the gateway. The copy of the manifests
above is the remedy, and it holds with or without an `fsGroup`: the init container runs after
Kubernetes has set the group on the volumes, and `chmod 0400` and `chown 65532:65532` are the
last things done to each file. The `chmod` comes first because the `CHOWN` capability does not let
root change the mode of a file it no longer owns.

The same file at `0400` starts:

```text
b10x-llm-gateway: listening on 0.0.0.0:8080
```

## Stop it

On `SIGTERM` the gateway turns `/ready` to `503`, so the Service stops routing to the pod, and
it answers every connection it had already accepted before it exits with status 0. The listener
stays open until the last of those connections is answered, so a connection made in the
meantime is not refused: it waits, and the gateway does not answer it. A relayed answer is not
bounded by the gateway: it lasts as long as the model talks, and the stop waits for it. A
connection already accepted that asks `/ready` is answered `503`.

Kubernetes sends `SIGTERM` when it starts the stop, and it removes the pod from its Service's
endpoints at about the same time, so a client may still be sent a request for a moment after
the signal. The `preStop` hook closes that gap: Kubernetes runs it **before** it sends
`SIGTERM`, so the gateway keeps serving while the endpoint removal spreads. Its `sleep` action
is done by the kubelet, because the image has no shell for a `sleep` command (the action needs
Kubernetes 1.30 or newer).

`terminationGracePeriodSeconds` covers the whole stop, the `preStop` sleep included, and
Kubernetes kills the container when it ends. Set it larger than the sleep plus the longest
relayed answer you allow. The gateway sets no limit on that answer. The manifest above sleeps
10 seconds and plans for an answer of 300, a figure of its own, so it sets 330: the sleep, the
answer and 20 seconds of margin. A request translated between wires
is bounded by 3600 seconds in all ([the relay](../concepts/relay.md)); raise the number if you
allow that.

The readiness probe on `/ready` keeps traffic off a pod that is not serving yet, and the
liveness probe on `/health` restarts one that has stopped answering. Neither needs a credential.

## Reach it from inside the cluster

The pod's port is named `http` and is `8080`. A client in the cluster reaches a pod at
`<pod-ip>:8080`, and a Service you write selects `app: llm-gateway` and targets that port. This
page names no Service type and no ingress: how, and whether, the gateway is exposed beyond the
cluster is yours to decide. A model call still needs the owner secret as a bearer, whatever
reaches the port.

## Scrape the counters

The document above sets `metrics_listen`, so the pod also listens on `9090`, the port named
`metrics`. It serves `GET /metrics` and nothing else, and it takes no owner secret, so the
Prometheus server never holds the secret that can make paid model calls
([Scrape the counters](scrape-the-counters.md#scrape-without-the-owner-secret)). It sets no
`metrics_bearer_file`, so that port is open to whatever reaches it: a NetworkPolicy keeps
everything but Prometheus off it. The one below lets any pod of the namespace reach `8080`,
where a model call still needs the owner secret, and only pods of the `monitoring` namespace
labelled `app.kubernetes.io/name: prometheus` reach `9090`:

```yaml title="network-policy.yaml"
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: llm-gateway
spec:
  podSelector:
    matchLabels:
      app: llm-gateway
  policyTypes: ["Ingress"]
  ingress:
    - from:
        - podSelector: {}
      ports:
        - port: 8080
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: monitoring
          podSelector:
            matchLabels:
              app.kubernetes.io/name: prometheus
      ports:
        - port: 9090
```

A NetworkPolicy is enforced only by a network plugin that implements it; on a cluster whose
plugin does not, set `metrics_bearer_file` as well, mounted from a Secret as the owner secret
is, and give Prometheus that token. Prometheus finds the pods on the `metrics` port with
`kubernetes_sd_configs` of role `pod`, or through a PodMonitor if you run the Prometheus
Operator.
