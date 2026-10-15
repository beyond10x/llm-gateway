---
format: aep.planning-md/3
id: story:in-pod-connectors
kind: story
status: draft
title: A gateway in a Kubernetes pod reaches the Runpod control plane through connectors
relations:
- decomposes: epic:hosting
- depends_on: story:kubernetes-deployment-guide
revision: 1
---
## Outcome

A gateway running in a Kubernetes pod can start and stop Runpod pods: the `connectors` CLI it calls
is present in the pod, and its keyring is read from a mounted Secret.

## Acceptance

1. The image of `story:release-image` (or a documented sidecar arrangement) provides the
   `connectors` binary at the path the deployment document names, from a pinned connectors
   release artifact.
2. The guide of `story:kubernetes-deployment-guide` gains a section mounting the connectors keyring
   from a Secret under the same file rules as the gateway's own files.
3. A test drives `ConnectorsRunpod` against the `connectors-fixture` binary placed at that path, as
   `crates/llm-runpod/tests/transport.rs` does today; no test reaches the Runpod API.

## Blocked

By `upstream-blocker:connectors-container-keyring`: no connectors release ships a container
artifact and a Secret-backed keyring.

## Scope (inferred)

`Dockerfile`, `crates/llm-runpod/tests/transport.rs`, `website/docs/guides/run-on-kubernetes.md`.
