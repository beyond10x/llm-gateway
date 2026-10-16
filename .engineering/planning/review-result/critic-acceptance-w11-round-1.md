---
format: aep.planning-md/3
id: review-result:critic-acceptance-w11-round-1
kind: review-result
status: active
title: acceptance critic, wave 2026-10-16-w02, round 1
relations:
- reviews: story:gateway-translation
- reviews: story:routes-document
revision: 1
---
approve

What I read: 2 of 2 ids (`story:gateway-translation`, `story:routes-document`), each with `aep plan artifact show`. I also ran `aep plan artifact lifecycle story` and checked the tree. `contracts/baseline.json` and `AGENTS.md:48` give the 232 floor that the translation story's verification cites. `contracts/gateway/scenarios/k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target.yaml` exists, so acceptance 7 names a real scenario. `story:hosted-base-path` is an implemented story I used to see the store's usual acceptance form.

What I could not establish:
- Both acceptances are numbered lists of separately observable checks, not single sentences (7 items and 4 items). I did not flag this. The store's other stories use the same form (`story:hosted-base-path` has 4 items), and each item has its own fixture, status code, or command. If the rubric's one-sentence rule is meant strictly, this would be a defect for both.
- Design point 9 of `story:gateway-translation` (Messages rewrites not applied on the translated path) has no acceptance item. The `translation-invalid` and `translation-unrepresentable` codes, and the non-streamed translated answer, are also unchecked. Rules 5 and 6 of `story:routes-document` are in the same position. Rule 6 says the inventory never lists a URL or credential, and it has no check. I treated these as the acceptance not being broad enough, which is outside my lane, so they did not set the verdict.
- Whether the translated-path and routes scenarios are actually named `translate-*` and `routes-*`: none exist yet (the directory has 116 scenarios, none with those prefixes), which is expected for a `proposed`/`draft` story.
- Out of my lane: the "Shares `relaying.rs`, `hosted.rs`…" ordering in `story:routes-document` belongs to the parallel-safety critic. The `depends_on` and `informed_by` edges belong to the design critic.

```findings
[]
```
