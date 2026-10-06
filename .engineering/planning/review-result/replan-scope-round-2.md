---
format: aep.planning-md/3
id: review-result:replan-scope-round-2
kind: review-result
status: active
title: Scope critic, spec-hardening epic and gateway-deployment split, round 2
relations:
- reviews: story:live-runpod-wiring
- reviews: epic:spec-hardening
- reviews: story:container-image
- reviews: story:public-model-listing
- reviews: story:provider-key-files
- reviews: story:gateway-deployment
revision: 1
---
needs-revision

story:live-runpod-wiring — the outcome sentence "Without the file, the binary refuses to start a pod rather than fall back to the emulator" traces to no matrix row, no sentence of the former parent and no acceptance item; cite its source or delete it — .engineering/planning/story/live-runpod-wiring.md:29-30

**Part 1, epic:spec-hardening.** No finding. Every promise traces to a story, and every one of the stories traces back to the epic.

- Gateway findings: the 25 missing, 9 spec-only and 1 unclear rows are 4-28, 29-37 and 38. `story:gateway-spec-declarations` claims them in `docs/verification/spec-hardening-gateway-review.md`.
- Hosting findings: the 35 missing, 3 unclear and 4 spec-only rows are 5-39, 40-42 and 43-46. `story:hosting-spec-declarations` claims them in `docs/verification/spec-hardening-hosting-review.md`.
- The unclaimed gateway and hosting rows are the plant (gateway row 3, hosting row 1), which the review marks caught, and the `fixed` rows (gateway 1-2, hosting 2-4).
- The spec-diff finding, with the `unknown` for an added domain, is claimed by `story:spec-diff-gate`.
- The hand mutant (`Requested -> Disowned`) is claimed by `story:hosting-spec-declarations` acceptance 5 and `story:hosting-lifecycle-declared`.
- The mutation audit and guard-analysis findings, and techniques 2-5, are named in the new "Not in scope" table, so they are not gaps.

**Part 2, the seven rows.** Each row lands in exactly one story.

- D1 and D3 are in `story:container-image`.
- R1 and R5 are in `story:public-model-listing`.
- K7 and B9 are in `story:provider-key-files`.
- B8 is in `story:gateway-deployment`.
- No other story or epic claims these rows (grepped; the only mentions are pointers, see below).
- The old outcome's seven pieces (image, profiles, setup page, model list, both key files, the call to the pod) are all claimed.
- I checked the specifics against llmgw `048ebd8`. The Dockerfile has distroless, non-root, `EXPOSE 8080`, an `llmgw` entrypoint and a revision label. `detect_client` matches the four User-Agent rules. The three profile headings match.
- `story:live-runpod-wiring` claims no row. It is traceable to the B8 note, "no production transport … implements `RelayTargets`, so the binary relays nothing" (`docs/llmgw-capability-matrix.md:132`), and to matrix line 22. `story:gateway-deployment` acceptance 5 records the deferral. Only the sentence above has no source.

**What I read:** 12 artifacts, with `aep plan artifact show` on the epic, the 4 spec stories, the 5 deployment stories, `story:runpod-production-transport` and `story:orphan-termination-obligations`. I also ran `aep plan artifact graph`, read the three `docs/verification/spec-hardening*.md` files and the matrix, and read the former `story:gateway-deployment` from `git show HEAD:`. Part 1 had 6 promises extracted and 6 traced. Part 2 had 7 rows and 7 landed.

**What I could not establish, and what is out of my lane (none of this set the verdict):**
- Out of lane (design): `story:gateway-observability` still says R1 and R5 belong to `story:gateway-deployment` (`.engineering/planning/story/gateway-observability.md:87` and `:97`), but they moved to `story:public-model-listing`.
- Out of lane (design): `story:gateway-deployment` acceptance 2 runs the binary against `EmulatedRunpod`, so the production binary or a test seam must accept an emulator.
- Out of lane (design): the matrix K27 note says "the story that composes a pool from the document decides what 0 means" for `idle_timeout_minutes`, and `story:gateway-deployment` acceptance 4 does not decide it.
- Out of lane (acceptance): the sentence in my finding has no acceptance item.
- I did not read any `review-result` artifact, so I could not check the revision against round 1. The `story:live-runpod-wiring` Why cites "the replan", which I treated as the drafter's reason rather than a parent promise.

```findings
- file: .engineering/planning/story/live-runpod-wiring.md
  line: 29
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome sentence 'Without the file, the binary refuses to start a pod rather than fall back to the emulator' traces to no matrix row, no sentence of the former parent and no acceptance item; cite its source or delete it"
```
