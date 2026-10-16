---
format: aep.planning-md/3
id: review-result:critic-scope-w11-round-2
kind: review-result
status: active
title: scope critic, wave 2026-10-16-w02, round 2
relations:
- reviews: story:gateway-translation
- reviews: story:routes-document
revision: 1
---
approve

Both stories cover the Milestone 2 part B goal, and nothing in either reaches past it.

**Round 1 finding, now fixed.** The promise "Publish exact supported ingress semantics, not a claim of complete vendor API equivalence" is now claimed by story:gateway-translation Acceptance 8. That item requires `docs/gateway.md` and a site page naming the accepted fields, the per-pair behaviour, the refusal codes and the known limits. It also requires each refused-field statement to be backed by a test that feeds the request to the decoder.

**Promises.** I extracted 12 from the two Contexts and Outcomes, and traced all 12 to an item.

| Promise (source) | Claimed by |
|---|---|
| Any of three wires to any served model, relay or translate (translation Outcome) | translation Acceptance 1 |
| Tool turns (translation Design) | translation Acceptance 2 |
| Refusals before dispatch (translation Design 5) | translation Acceptance 3 and 9 |
| Encoder refusals (translation Design 6) | translation Acceptance 4 |
| Cancellation (translation Design 8) | translation Acceptance 5 |
| Listing per model (translation Design 10) | translation Acceptance 6 |
| Relay without a translator keeps `wire-not-served` (translation Design 1) | translation Acceptance 7 |
| "No downstream tool execution" | A constraint. The translation Outcome and Design say nothing about running tools. |
| Routes document load and per-route aliases (routes Outcome) | routes Acceptance 1 and 2 |
| Credentials and `base_url` rules | routes Acceptance 3 and 4 |
| `[models.*]` and `[hosted.*]` keep working beside routes (routes Context) | routes Acceptance 5, so the coexistence gap I left to another critic in round 1 is closed |
| Cross-wire translation of routes | routes Outcome, routes Acceptance 1, `depends_on story:gateway-translation` |

**Gap, overlap and narrowing checks.**
- **Reach beyond the parent:** none. Design 9 (no effort mapping or `count_tokens` on the translated path) is a consequence of the translation Outcome, not new work. `GET /v1/routes` is pre-existing, with evidence under `.engineering/evidence/story/ingress-auth-optional/`.
- **Overlap:** none. The two stories both touch `GET /v1/models`, but translation lists wires per configured model and routes lists route aliases.
- **Narrowing:** every dropped item is named under "Not in scope" in one of the two stories. Those are usage records, widening the ingress subset, route fallback, `env:` and `keychain:` secrets, and reloading the routes document.

**What I read.** 4 artifacts: `epic:gateway`, both stories and `review-result:critic-scope-w11-round-1`. I ran `aep plan artifact show` on the first three and on the review result, `aep plan artifact graph`, and `git grep /v1/routes`.

**What I could not establish.**
- **Epic too generic:** `epic:gateway` says only "supplies its part of the full foundation", so the Contexts are the only source for the part B goal.
- **Out of my lane:** the artifact `scope` fields list fewer files than the body Scope sections (the design and parallel-safety critics' lane).

```findings
[]
```
