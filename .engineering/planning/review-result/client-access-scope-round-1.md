---
format: aep.planning-md/3
id: review-result:client-access-scope-round-1
kind: review-result
status: active
title: Scope critic, epic:client-access, round 1
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
needs-revision

- `epic:client-access` — the design says "which of llmgw's profiles (32768 or 65536 tokens) leaves enough room is measured by `story:client-qualification`", but that story's acceptance never records a context-window measurement (its "Why" lists three facts: thinking, reasoning include and `apply_patch`, cold-start time), so `story:client-qualification` should take it — `docs/design/runpod-clients.md:187-188` (story: `.engineering/planning/story/client-qualification.md:28-44`)
- `epic:client-access` — the epic says `story:public-model-listing` "renders the `ClientProfile` settings at `GET /`", and the design says `GET /` "prints steps 5 to 7" (step 7 is Loom), yet the `## Client profiles` section covers only Codex and Claude Code and says nothing about where the Loom profile is rendered, so `story:public-model-listing` should say it or record the omission — `.engineering/planning/epic/client-access.md:44` (design `docs/design/runpod-clients.md:297-298`; story `.engineering/planning/story/public-model-listing.md:94-99`)

**What you read:** 8 artifacts (`epic:client-access`, the five new stories, `story:public-model-listing` plus its diff, and `docs/design/runpod-clients.md` and `spec/domains/clients.yaml` in full). Commands: `aep plan artifact show epic:client-access`, `cat` of the six story files, `aep plan artifact graph`, `aep plan artifact relations`, `git diff` on `public-model-listing.md`, and greps of the other stories for "240", "context window" and "hold budget". I extracted 14 promises from the epic and the design (the three clients, one gateway and token, on-demand start and idle reap, the five-story table, the `GET /` profiles, Done-when, three exclusions, context-window measurement, Loom profile at `GET /`, the D2 and D3 decisions). I traced 12 to an item or to the existing chain, or to a named exclusion. The two findings above are the two I could not trace.

**What I could not establish:**
- Whether any story claims the design's "recommended hold is 240 s" (`docs/design/runpod-clients.md:164-165`). It reads as a recommendation, not a promise, and `story:cold-start-hold` does not mention it. I did not count it as a gap.
- Out of my lane: the acceptance criteria of `story:pod-proxy-tls` and `story:model-tool-calling` look checkable. I did not judge the "no trust-root option" criterion (`pod-proxy-tls.md:57-58`), which adds a constraint the epic does not state. That is a design question.
- Nothing in the set claims a promise already covered by `epic:gateway` or `epic:hosting`. The graph shows the relay-chain stories exist and the two `dependency-blocker`s the epic names are present.

```findings
- file: docs/design/runpod-clients.md
  line: 187
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "epic:client-access: the design promises that which of llmgw's profiles (32768 or 65536 tokens) leaves enough room is measured by story:client-qualification, but that story's acceptance and Why record no context-window measurement, so story:client-qualification should claim it"
- file: .engineering/planning/epic/client-access.md
  line: 44
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "epic:client-access: the epic says story:public-model-listing renders the ClientProfile settings at GET / and the design says GET / prints steps 5 to 7 (Loom is step 7), but the Client profiles section covers only Codex and Claude Code and says nothing about where the Loom profile is rendered, so story:public-model-listing should claim it or record the omission"
```
