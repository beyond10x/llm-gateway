---
format: aep.planning-md/3
id: review-result:replan-design-round-1
kind: review-result
status: active
title: Design critic, spec-hardening epic and gateway-deployment split, round 1
relations:
- reviews: story:provider-key-files
- reviews: story:public-model-listing
- reviews: story:gateway-spec-declarations
- reviews: story:spec-diff-gate
- reviews: story:runpod-production-transport
- reviews: story:gateway-deployment
- reviews: epic:spec-hardening
revision: 1
---
needs-revision

story:provider-key-files — acceptance 3 describes "the vLLM key each pod is started with", an outcome that exists only after story:gateway-deployment composes `RunpodPool`, and the edge runs the other way (deployment depends on this story), so B9 cannot be shown done here; reword it to what the binary holds at startup, or move the pod-side observation to story:gateway-deployment acceptance 1 — .engineering/planning/story/provider-key-files.md:49

story:provider-key-files — depends_on story:gateway-spec-declarations has no reason in the body (the only shared file is spec/domains/deployment.yaml), and it puts all of epic:gateway behind the 25-rule hardening story; either write the shared-file reason or split the deployment.yaml half of gateway-spec-declarations so this story waits only on that half — .engineering/planning/story/provider-key-files.md:10

story:public-model-listing — depends_on story:gateway-spec-declarations has no reason in the body (shared files are spec/domains/gateway.yaml, docs/gateway.md and checks/conformance/src/gateway.rs); write the reason so the edge reads as a trade-off against splitting the shared surface — .engineering/planning/story/public-model-listing.md:10

story:spec-diff-gate — no edge or sentence orders it against story:gateway-spec-declarations and story:hosting-spec-declarations, which delete 9 and 4 `spec-only` declarations that this gate classifies as breaking; all three sit in the same wave on ess-0-53 alone, so merge order decides whether the hardening needs acknowledgements; add depends_on edges from the gate to both, or state in those two stories that their deletions are acknowledged in contracts/diff-acknowledgements.yaml — .engineering/planning/story/gateway-spec-declarations.md:44

story:runpod-production-transport — depends_on story:hosting-spec-declarations has no reason in the body (shared: spec/domains/runpod.yaml, docs/hosting.md), and the Scope still says rows K7, B8, B9 belong to story:gateway-deployment although K7 and B9 moved to story:provider-key-files — .engineering/planning/story/runpod-production-transport.md:71

What I read: 20 artifacts (the 5 spec-hardening items, the 4 split stories, runpod-production-transport, the usage decoder blocker, usage-records, gateway-translation, ess-0-53, modal-hosting, cold-start-hold, hosted-endpoints, orphan-termination-obligations) with `aep plan artifact show`, `relations`, `graph` (about 130 edges, including edges to artifacts outside the set) and `validate` (valid). There is no cycle. The longest `depends_on` path has 10 stories: ess-0-53, gateway-spec-declarations, provider-key-files, gateway-deployment, cold-start-hold, gateway-observability, hosted-endpoints, target-fallback, usage-records, gateway-translation.

What I could not establish:
- Whether story:hosting-lifecycle-declared collides with story:orphan-termination-obligations. The orphan story lists `machine.rs` and `hosting.yaml` (inferred) and has no acceptance yet; the lifecycle story pins the 8 phases and `TRANSITIONS` exactly. No edge links them, and I could not tell which direction it would run.
- Whether story:gateway-translation needs story:modal-hosting. The only reason written is that llm's record had the edge, while its Context calls hosting optional. Its "Not established" paragraph (`gateway-translation.md:479`) still says no edge came across.
- Out of my lane (parallel safety): story:public-model-listing and story:gateway-deployment both edit `spec/domains/gateway.yaml` and `checks/conformance/src/gateway.rs` with no edge between them. story:orphan-termination-obligations, story:hosting-spec-declarations, story:runpod-production-transport and story:hosting-lifecycle-declared all share `spec/domains/runpod.yaml`, `spec/domains/hosting.yaml` and `docs/hosting.md`. The 10 waves ignore the integration files listed in AGENTS.md, so they understate these collisions.
- Out of my lane (scope): story:hosted-endpoints still cites rows "B8, B9" for the header that story:gateway-deployment adds.

```findings
- file: .engineering/planning/story/provider-key-files.md
  line: 49
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 describes the vLLM key each pod is started with, an outcome that exists only once story:gateway-deployment composes RunpodPool, while the edge runs from deployment to this story; reword it to what the binary holds at startup or move the pod-side observation to story:gateway-deployment acceptance 1"
- file: .engineering/planning/story/provider-key-files.md
  line: 10
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "depends_on story:gateway-spec-declarations has no reason in the body (shared file spec/domains/deployment.yaml) and puts all of epic:gateway behind the hardening story; write the shared-file reason or split the deployment.yaml half of gateway-spec-declarations so this story waits only on that half"
- file: .engineering/planning/story/public-model-listing.md
  line: 10
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "depends_on story:gateway-spec-declarations has no reason in the body (shared files spec/domains/gateway.yaml, docs/gateway.md, checks/conformance/src/gateway.rs); write the reason so the edge reads as a trade-off against splitting the shared surface"
- file: .engineering/planning/story/gateway-spec-declarations.md
  line: 44
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "no edge or sentence orders story:spec-diff-gate against the two declaration stories whose spec-only deletions it classifies as breaking, and all three share a wave on ess-0-53 alone; add depends_on edges from story:spec-diff-gate to both, or state in both stories that the deletions are acknowledged in contracts/diff-acknowledgements.yaml"
- file: .engineering/planning/story/runpod-production-transport.md
  line: 71
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "depends_on story:hosting-spec-declarations has no reason in the body (shared spec/domains/runpod.yaml, docs/hosting.md), and the Scope still says rows K7, B8, B9 belong to story:gateway-deployment although K7 and B9 moved to story:provider-key-files"
```
