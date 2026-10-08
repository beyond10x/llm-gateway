---
format: aep.planning-md/3
id: review-result:client-access-scope-round-2
kind: review-result
status: active
title: Scope critic, epic:client-access, round 2
relations:
- reviews: epic:client-access
- reviews: story:model-tool-calling
- reviews: story:pod-proxy-tls
- reviews: story:client-qualification
- reviews: story:claude-code-wire
- reviews: story:loom-qualification
- reviews: story:public-model-listing
revision: 1
---
approve

I traced both round-1 findings to the revised bodies and found nothing new in scope. I extracted 15 promises from `epic:client-access` and `docs/design/runpod-clients.md`. All 15 trace to an item, to a story already in the store, or to a named exclusion.

Round-1 follow-up, both fixed:
- **Context window.** `story:client-qualification` now carries it in acceptance 4 (the model's `context_window` and the largest `usage.input_tokens`). Its Why lists the 32768 vs 65536 question.
- **Loom profile.** `story:public-model-listing` `## Client profiles` criterion 10 now renders the Loom catalog lines (design § 5 step 7) in the plain-text and HTML answers of `GET /`.

**What I read:** 8 artifacts and 2 files. The artifacts were `epic:client-access`, the five stories, `story:public-model-listing`, and `review-result:client-access-scope-round-1`. The files were `docs/design/runpod-clients.md` in full and `story:cold-start-hold` (grepped for "240", "hold" and "budget"). Commands: `aep plan artifact show` on each, `aep plan artifact graph`, and `git log` / `git status`. I did not re-run `aep plan artifact relations` or `kinds` this round.

**What I could not establish:**
- Nothing claims the design's "recommended hold is 240 s" (`docs/design/runpod-clients.md:163-165`). The design marks it as an inferred recommendation, not a promise, and `story:cold-start-hold` does not mention it. I left it out as in round 1.
- Criterion 11 of `public-model-listing` withholds a profile for a model whose tool calling is `Absent`, "for that client". Design § 4 (`:182`) names only Claude Code and Codex for that rule, so the criterion's scope for the Loom profile is unclear. I did not count it as reach beyond the parent. It is a question for the design and acceptance critics.
- `story:pod-proxy-tls` criterion 3 (no trust-root key or option) goes slightly beyond the epic's "verified TLS". The story's Outcome ties it to the platform trust roots, so I judged it a design question, not scope.
- Under D1 option A, the connectors and llm changes are not held as `dependency-blocker`s. The store holds them under the open `decision-blocker:runpod-control-plane`, outside this set, so I did not count it as a gap.

```findings
[]
```
