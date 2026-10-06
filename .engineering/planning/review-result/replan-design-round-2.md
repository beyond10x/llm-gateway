---
format: aep.planning-md/3
id: review-result:replan-design-round-2
kind: review-result
status: active
title: Design critic, spec-hardening epic and gateway-deployment split, round 2
relations:
- reviews: story:live-runpod-wiring
- reviews: story:orphan-termination-obligations
- reviews: story:gateway-translation
- reviews: epic:spec-hardening
- reviews: story:gateway-deployment
revision: 1
---
needs-revision

story:live-runpod-wiring — its outcome hands the transport only the Runpod key, while story:runpod-production-transport takes each model's vLLM key as a value for the readiness probe (B6) and the other split stories never wire it, so the key's hand-off to the transport has no owner; state it in this story's outcome or acceptance — .engineering/planning/story/live-runpod-wiring.md:28 and .engineering/planning/story/runpod-production-transport.md:53
story:orphan-termination-obligations — it declares depends_on story:hosting-spec-declarations and depends_on story:runpod-production-transport, but unlike every other edge in the set it has no `## Depends on` section, so no reason sits beside either edge. If the reason is the shared `spec/domains/runpod.yaml` DEFERRED header, `docs/hosting.md` and `checks/conformance/src/runpod.rs`, write that down. The alternative is to split that surface so the edge goes away — `.engineering/planning/story/orphan-termination-obligations.md:10-11` (headings at :37, :42 and :48; no `## Depends on`)
story:gateway-translation — it needs-first depends on story:modal-hosting, but its own Context calls hosting "optional" and its `## Depends on` gives only provenance ("llm's record ... depended on it"). Meanwhile the target is a draft whose acceptance requires a live Modal deployment result, so translation would wait on it with no stated need. State what translation needs from Modal, or record the edge as informed_by — `.engineering/planning/story/gateway-translation.md:10`, `:42`, `:101-103`

**What I read.** 21 draft stories plus the epic, the blocker, the two story:ess-* dependencies and story:modal-hosting, all in full with `aep plan artifact show`. I also ran `aep plan artifact relations`, `aep plan artifact graph` (all edges, including those outside the set, review-result nodes excluded), `aep plan artifact validate` (valid, 49 artifacts) and `aep plan artifact waves --kind story --status draft` (12 waves). I read no review-result artifact.

**Walked.** I followed about 75 depends_on, decomposes and blocks edges, outside the set as well as inside it. There is no cycle. The longest declared chain is ess-0-53 → gateway-spec-declarations → provider-key-files → gateway-deployment → cold-start-hold → gateway-observability → hosted-endpoints → target-fallback → usage-records → gateway-translation. Every edge in the new set except the two named above carries a written reason, and all of those reasons are shared files or a named dependency, which the rubric treats as a trade-off and not a defect. The blocker's `blocks` edge into story:usage-records points the right way.

**What I could not establish.**
- **Out of my lane (parallel safety).** Several pairs touch `crates/llm-gateway-cli/src/serve.rs` and `spec/domains/gateway.yaml` with no edge between them, for example story:gateway-deployment and story:public-model-listing. The wave tool separates them anyway (waves 4 and 5). I did not judge this.
- **Out of my lane (acceptance).** story:orphan-termination-obligations has no acceptance written. Story:gateway-deployment's process test "against `EmulatedRunpod`" implies an emulator mode in the shipped binary, and story:live-runpod-wiring says "rather than fall back to the emulator". I did not judge whether that mode is specified.
- **Unestablished: no edge from story:live-runpod-wiring to story:orphan-termination-obligations.** `docs/hosting.md:316-324` says orphan terminations bypass the controller and the budget ledger. I could not tell from the store whether real pods in the binary need that closed first. No ledger is wired in the binary today, so I left it as a hypothesis.
- **Unestablished: story:modal-hosting has no edge to story:hosting-spec-declarations.** It is unassessed (no scope) and would supply the hosting contract that story hardens.
- **Not examined: the tail edges.** story:cold-start-hold and story:gateway-observability have no `## Depends on` section for their outbound edges. They sit outside the set I was handed, so I did not judge them.

```findings
- file: ".engineering/planning/story/live-runpod-wiring.md"
  line: 28
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its outcome hands the transport only the Runpod key, while story:runpod-production-transport takes each model's vLLM key as a value for the readiness probe (B6) and the other split stories never wire it, so the key's hand-off to the transport has no owner; state it in this story's outcome or acceptance"
- file: ".engineering/planning/story/orphan-termination-obligations.md"
  line: 10
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it declares depends_on story:hosting-spec-declarations and depends_on story:runpod-production-transport but has no '## Depends on' section, so neither edge has a written reason; if it is the shared runpod.yaml DEFERRED header, docs/hosting.md and checks/conformance/src/runpod.rs, write that down, or split that surface so the edge goes away"
- file: ".engineering/planning/story/gateway-translation.md"
  line: 10
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it needs-first depends on story:modal-hosting, but its own Context calls hosting optional and its Depends on section gives only provenance, while the target requires a live Modal deployment result; state what translation needs from Modal, or record the edge as informed_by"
```
