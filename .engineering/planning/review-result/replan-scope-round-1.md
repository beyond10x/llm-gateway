---
format: aep.planning-md/3
id: review-result:replan-scope-round-1
kind: review-result
status: active
title: Scope critic, spec-hardening epic and gateway-deployment split, round 1
relations:
- reviews: epic:spec-hardening
- reviews: story:container-image
- reviews: story:gateway-spec-declarations
- reviews: story:hosting-spec-declarations
- reviews: story:spec-diff-gate
- reviews: story:hosting-lifecycle-declared
- reviews: story:provider-key-files
- reviews: story:public-model-listing
- reviews: story:gateway-deployment
revision: 1
---
needs-revision

epic:spec-hardening — "the IR declares 0 guards, 0 invariants and 0 transitions" is listed as a finding, but only the hosting transitions are claimed (by story:hosting-lifecycle-declared). Guards, invariants and the gateway commands' single `observed` outcome are neither claimed by a child nor excluded by name in the epic. The epic body should either exclude them by name or add them to a child, most naturally story:gateway-spec-declarations. — .engineering/planning/epic/spec-hardening.md:28-29

story:container-image — row D1 reads "distroless, non-root, port 8080, `ENTRYPOINT llmgw`, source revision label". The outcome and acceptance 1 cover base image, user, port and entrypoint but never the source revision label. The row would be marked `covered` with that part silently dropped. — .engineering/planning/story/container-image.md:32 (row at docs/llmgw-capability-matrix.md:169)

story:container-image — row D3 is "two proven model profiles … and the weight-cache block" (`docs/model-profiles.md:88-103`, cited separately from the two profiles). Acceptance 2 and the outcome cover only "both profiles", so the weight-cache block is claimed by no story. The matrix gap list says "The proven model profiles and the weight-cache block exist as llm-gateway model declarations (D3)". — .engineering/planning/story/container-image.md:35 (row at docs/llmgw-capability-matrix.md:171, gap list at :200)

**What you read:** 9 artifacts, all with `aep plan artifact show`: epic:spec-hardening, its four child stories, story:gateway-deployment, provider-key-files, public-model-listing and container-image. I also read epic:gateway, `aep plan artifact graph`, the original gateway-deployment body (`git show HEAD:…/gateway-deployment.md`), `docs/verification/spec-hardening.md` and matrix rows D1, D3, R1, R5, K7, B8, B9.

**Counts:**
- Part 1: I extracted 11 promises from the epic and traced 10 to a child. The 11th is the guards/invariants finding.
- Part 2: all 7 rows land in exactly one story (D1 and D3 in container-image, R1 and R5 in public-model-listing, K7 and B9 in provider-key-files, B8 in gateway-deployment). No other story in the store names any of them. The two findings above are about parts of rows D1 and D3, not about row placement.

**What I could not establish:**
- The record's "three contradictions … fixed in the hardening change" does not match its own tally of 2 gateway plus 2 hosting contradicts. I did not check which of the four was fixed, so I took both stories' "no `contradicts`" re-run acceptance as covering it.
- Techniques 2–5 appear under "Not run" in the record, not as findings, so I did not count them as gaps. The epic does not exclude them by name.
- Out of my lane:
  - hosting-spec-declarations acceptance 5 and hosting-lifecycle-declared acceptance 2 both drive `Requested -> Disowned` (design critic).
  - public-model-listing edits `docs/gateway.md` and `gateway.yaml`, which gateway-spec-declarations also changes (parallel-safety critic).
  - B9's outcome says "names the source" while the story title says "from trusted files" (acceptance critic).

```findings
- file: .engineering/planning/epic/spec-hardening.md
  line: 28
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the finding \"the IR declares 0 guards, 0 invariants and 0 transitions\" is claimed only for hosting transitions (story:hosting-lifecycle-declared); guards, invariants and the gateway commands' single observed outcome are neither claimed by a child nor excluded by name, so the epic body should exclude them or story:gateway-spec-declarations should take them"
- file: .engineering/planning/story/container-image.md
  line: 32
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "row D1 promises a \"source revision label\" and neither the outcome nor acceptance 1 claims it, so D1 would be marked covered with part of the row dropped"
- file: .engineering/planning/story/container-image.md
  line: 35
  category: scope
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "row D3 promises \"the weight-cache block\" (model-profiles.md:88-103) and acceptance 2 claims only the two profiles, so the weight-cache block is claimed by no story"
```
