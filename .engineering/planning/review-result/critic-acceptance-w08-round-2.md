---
format: aep.planning-md/3
id: review-result:critic-acceptance-w08-round-2
kind: review-result
status: active
title: acceptance critic, wave 2026-10-15-w01 new stories, round 2
relations:
- reviews: story:effort-default
- reviews: story:max-tokens-clamp
revision: 1
---
needs-revision

story:effort-default — criteria 2 and 3 disagree on a request that carries `chat_template_kwargs.reasoning_effort` but no `output_config.effort`: criterion 2 ("a request with no `output_config.effort`") inserts the declared value, criterion 3 ("a request that already carries an effort") leaves it unchanged, so the acceptance does not say which effort key counts as "carrying an effort" — .engineering/planning/story/effort-default.md:71

What I read: 2 of 2 artifacts (`story:effort-default`, `story:max-tokens-clamp`), in full via `aep plan artifact show` for each. I also ran `aep plan artifact kinds`, `aep plan artifact lifecycle story` and `aep plan artifact findings` for both, and read `story:claude-code-wire` for the fixture and the effort mapping the acceptances rely on. I checked the named symbols with `git grep`: hosted `context_window`, Runpod `max_model_len`, `config:value`, "llmgw's 13" counters, and no existing `ContextExceeded`.

Round-1 acceptance findings: the store holds none (the round-1 results are scope and parallel-safety only), so this is the first acceptance pass.

`story:max-tokens-clamp` has no finding. Its acceptance states the before and after of the clamp, tests the `w-n-1`, `w-n` and `w-n+1` boundaries, and makes the refusal and the failure paths observable. Every artifact it names exists or is declared as new.

What I could not establish:
- Whether `story:max-tokens-clamp` criterion 4's "never wakes a pod the relay would not have woken" is pinned by a test. No test is named for it, since "a test each" covers only the count failures. I left this as an unease, not a finding.
- The "prompt is too long" source for criterion 3 of `story:max-tokens-clamp` is a "cite the source used" instruction, not yet a citation. That is a draft-stage gap, not a defect.
- Out of my lane: the `epic:client-access` table and Done-when still count five stories, and that scope finding from round 1 is not mine to judge. The shared-surface ordering edge is parallel-safety's.

```findings
- file: .engineering/planning/story/effort-default.md
  line: 71
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "criteria 2 and 3 disagree on a request carrying chat_template_kwargs.reasoning_effort but no output_config.effort: criterion 2 ('a request with no output_config.effort') inserts the declared value while criterion 3 ('a request that already carries an effort') leaves it unchanged, so the acceptance does not say which effort key counts as carrying an effort"
```
