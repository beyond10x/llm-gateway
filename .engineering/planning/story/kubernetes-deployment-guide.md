---
format: aep.planning-md/3
id: story:kubernetes-deployment-guide
kind: story
status: implemented
title: A guide runs the released image on Kubernetes with the file rules unchanged
relations:
- decomposes: epic:gateway
- depends_on: story:release-image
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/llm-gateway-cli/tests/trusted.rs
- confidence: inferred
  path: website/docs/guides/run-on-kubernetes.md
- confidence: inferred
  path: website/docs/index.md
- confidence: inferred
  path: website/sidebars.ts
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 7}
- {from: "proposed", to: "active", at: "2026-10-16T20:21:00Z", actor: "human:timo", revision: 8}
- {from: "active", to: "implemented", at: "2026-10-17T01:21:02Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

The documentation site has one guide that runs the released image on Kubernetes with the gateway's
file rules unchanged: Secrets mounted so the trusted-file reader accepts them, the fsGroup case
handled by copying, and a stop that drains before the process exits.

## Acceptance

1. A guide page `website/docs/guides/run-on-kubernetes.md` holds a complete manifest set (a
   ConfigMap with the deployment document, a Secret, a Deployment) for the image of
   `story:release-image`. Each Secret file is mounted with `defaultMode: 0400` (`256`) and `subPath`,
   and the container runs as the image's `nonroot` user, the files' owner.
2. The page says that a pod `fsGroup` makes Kubernetes set the group read bit (`0440`), that the
   gateway refuses such a file (`owner-secret:unsafe-mode`, pasted from a run), and that the reader
   is not relaxed. Its remedy is an init container that copies each Secret file into an
   `emptyDir` with mode `0400` owned by the runtime user, and the page shows that init container.
3. A test runs the binary against a key file of mode `0440` and asserts the refusal the page pastes;
   the page's pasted line equals the test's expected line, so a change to either fails the
   docs check.
4. The page documents the stop: on SIGTERM the gateway begins its drain (`/ready` answers `503`)
   and answers every connection it already accepted before it exits. It shows a `preStop` hook with
   the `sleep` action (the image has no shell), a readiness probe on `/ready`, a liveness probe on
   `/health`, and a `terminationGracePeriodSeconds` larger
   than the preStop sleep plus the longest relayed answer the operator allows. No code changes,
   unless writing the page shows the documented behaviour is not what the binary does; then that
   is a finding, recorded on this story, not a silent fix.
5. The page names no Service type and no ingress: it shows how a client inside the cluster reaches
   the pod's port and leaves exposure to the operator.
6. `website/` builds (`npm --prefix website run build`) and `llm-gateway-docs generate --check`
   passes with the page linked from the guides index.

## Not in scope

`/metrics` and a private CA (left out by decision); the in-pod `connectors` CLI
(`story:in-pod-connectors`); a Helm chart.

## Scope (inferred)

`website/docs/guides/run-on-kubernetes.md` (new), `website/sidebars.*`, `website/docs/index.md`,
`crates/llm-gateway-cli/tests/binary.rs` or `tests/trusted.rs` (the 0440 refusal), `CHANGELOG.md`.
