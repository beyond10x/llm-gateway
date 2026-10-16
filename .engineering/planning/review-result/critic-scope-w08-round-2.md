---
format: aep.planning-md/3
id: review-result:critic-scope-w08-round-2
kind: review-result
status: active
title: scope critic, wave 2026-10-15-w01 new stories, round 2
relations:
- reviews: story:effort-default
- reviews: story:max-tokens-clamp
revision: 1
---
approve

Both round-1 findings are fixed. I found nothing new.

- `story:effort-default` now claims only the insertion of a hosted model's declared effort when the request names none. Its Outcome and Acceptance 3 say the `high`/`max` to `xhigh` mapping is `story:claude-code-wire`'s and is relied on unchanged, so only `story:claude-code-wire` claims that mapping (its Acceptance 4).
- `epic:client-access` lists seven stories, including both new ones, and its "Done when" reads "Each of the seven stories" (`.engineering/planning/epic/client-access.md:58`).

**What you read:** 5 artifacts, using `aep plan artifact show` for `epic:client-access`, `story:effort-default`, `story:max-tokens-clamp`, `story:claude-code-wire` and `review-result:critic-scope-w08-round-1`. I also ran `aep plan artifact graph`, `git grep context_window`, and read the scope procedure and the rubric. I did not run `kinds` or `relations`.

Promises extracted from the consumer report and epic: 3 for the report, traced to an item: 3.
- Map `low`/`medium`/`high`/`max` to values vLLM accepts, or pass a thinking budget. This is claimed once, by `story:claude-code-wire` Acceptance 4 (the mapping). The "thinking budget" alternative was not taken; it was an "or" in the report.
- Effort dropped, so vLLM's server default applies. `story:effort-default` Acceptance 2 claims this.
- Clamp `max_tokens` to the context window minus prompt tokens. `story:max-tokens-clamp` Acceptance 2 and 3 claim this.

The epic's "Done when" now covers both new stories, so they are claimed by the epic as well. The spec-first and recorded-fixture requirements appear in both stories as an "ESS first" section and fixture-pod acceptances.

**What I could not establish:**
- **Runpod models.** The Runpod effort and `max_model_len` behaviour is stated only in the stories' Why and Acceptance text. I did not check it against the crates, and it has no bearing on scope.
- **`count_tokens` as new behaviour.** `story:max-tokens-clamp` adds a `count_tokens` call and a new `ContextExceeded` refusal. I treat the refusal as the necessary `w - n < 1` case of the clamp, and the count as how "minus prompt tokens" is obtained. Both are traceable to the report. The "no metric series" criterion limits the extra reach.
- **Out of my lane.** Whether the "0.7.0 drops effort" premise is a gateway fault is not settled. `story:claude-code-wire` records it as open in its record item 7. That is for the design and acceptance critics, and it did not set my verdict.

```findings
[]
```
