---
format: aep.planning-md/3
id: review-result:critic-scope-w11-round-1
kind: review-result
status: active
title: scope critic, wave 2026-10-16-w02, round 1
relations:
- reviews: story:gateway-translation
- reviews: story:routes-document
revision: 1
---
needs-revision

I read epic:gateway first, then both stories in full, then the graph. Part B of the operator's plan splits into translation and routes-document, and both stories have it covered. I extracted 9 promises from the two stories' Context and Outcome. I traced 8 of them to an item and could not trace 1. The epic is generic ("supplies its part of the full foundation"), so the Context sections are the only source for the part B goal.

Findings:

story:gateway-translation — The operator's Context sentence "Publish exact supported ingress semantics, not a claim of complete vendor API equivalence" is claimed by no Outcome or Acceptance item. The only trace is a "translation page" in the Scope list. Add a published-subset outcome and a matching acceptance, for example a docs page that names the supported fields and the refusal codes. — .engineering/planning/story/gateway-translation.md:65 (promise), :145 (Scope-only mention)

Checked and not findings:
- **Gaps:** Translation claims the first half of part B and routes-document claims the second. story:hosted-base-path (.engineering/planning/story/hosted-base-path.md:130) names these two as Milestone 2 part B.
- **Overlap:** Translation and routes both touch `GET /v1/models`, but not the same outcome. Translation lists wires for configured models (Acceptance 6). Routes lists route aliases (Acceptance 2) and depends on translation. Design 7 carries usage counters inside the translated answer, while story:usage-records keeps the usage records and metrics.
- **Reach beyond the parent:** None found.
- **Narrowed promises:** Env and keychain secrets, route fallback, and routes without a model are named under routes-document's "Not in scope". Widening llm's ingress subset is named under translation's "Not in scope".

Out of my lane, and they did not set the verdict:
- routes-document Context says `[models.*]` and `[hosted.*]` keep working beside routes (routes-document.md:51), but no acceptance shows coexistence. That belongs to the acceptance critic.
- The translation body's Scope lists `server.rs`, `status.rs`, `website/docs/` and `CHANGELOG.md`, which the artifact's scope field omits. That belongs to the design and parallel-safety critics.

```findings
[{"file": ".engineering/planning/story/gateway-translation.md", "line": 65, "category": "scope", "severity": "warning", "message": "story:gateway-translation: the operator promise \"Publish exact supported ingress semantics, not a claim of complete vendor API equivalence\" is claimed by no Outcome or Acceptance item; only a Scope bullet at line 145 mentions a translation page. Add a published-subset outcome and acceptance."}]
```
