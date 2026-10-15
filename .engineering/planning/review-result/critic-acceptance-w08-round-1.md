---
format: aep.planning-md/3
id: review-result:critic-acceptance-w08-round-1
kind: review-result
status: active
title: Acceptance critic, wave 2026-10-15-w01 new stories, round 1
relations:
- reviews: story:effort-default
- reviews: story:max-tokens-clamp
revision: 1
---
needs-revision
story:effort-default — criterion 3 points to "criterion 1 of the outcome", which does not exist (the Outcome has no numbered criteria and acceptance criterion 1 is the hosted config key), so the mapping it keeps has to be read from the parenthesis alone; the acceptance should name the mapping itself — .engineering/planning/story/effort-default.md:74
story:effort-default — criterion 2 sets the inserted `output_config.effort` "to it" (the declared value) while criterion 3 maps `high` to `xhigh`, and a Runpod model may declare `high` (`REASONING_EFFORTS` check, crates/llm-gateway-cli/src/config.rs:657-661), so a no-effort request to that model could reach the pod as `high` or `xhigh`, and the acceptance does not say which — .engineering/planning/story/effort-default.md:68
story:max-tokens-clamp — criterion 1 uses a window `w` without saying which window applies, and the "no clamp when a hosted model declares none" case from the ESS-first section appears in no acceptance criterion, not even among the no-count cases of criterion 4, so a hosted model with no `context_window` can be clamped or counted and still pass — .engineering/planning/story/max-tokens-clamp.md:63
story:max-tokens-clamp — criterion 3 says a failed count "is counted in the counters" but names no counter, and the metrics are a fixed set of series (crates/llm-gateway/src/metrics.rs:4), so nobody can say which value to read before and after — .engineering/planning/story/max-tokens-clamp.md:73

What I read: 3 artifacts (story:effort-default, story:max-tokens-clamp, and story:claude-code-wire for context), each in full with `aep plan artifact show`. I checked the tree with `git grep` for `reasoning_effort`, `context_window`, `max_model_len`, `ContextExceeded` and the metrics counters. I did not run `aep plan artifact kinds` or `lifecycle`, because I used no status or kind names.

What I could not establish:
- Whether the claim that vLLM v0.27.1 serves `/v1/messages/count_tokens` is true. That is a source claim outside the tree.
- Overlap between effort-default criterion 3 and claude-code-wire criterion 4 (both pin `max` to `xhigh`) is out of my lane, so it did not set my verdict. It belongs to plan-critic-scope and plan-critic-parallel-safety.
- I did not treat the multi-part numbered criteria (for example the docs plus CHANGELOG items) as a "more than one statement" defect. The active sibling story uses the same numbered form.

```findings
- file: .engineering/planning/story/effort-default.md
  line: 74
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "criterion 3 points to \"criterion 1 of the outcome\", which does not exist (the Outcome has no numbered criteria and acceptance criterion 1 is the hosted config key), so the mapping it keeps has to be read from the parenthesis alone; the acceptance should name the mapping itself"
- file: .engineering/planning/story/effort-default.md
  line: 68
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "criterion 2 sets the inserted output_config.effort \"to it\" (the declared value) while criterion 3 maps high to xhigh, and a Runpod model may declare high (crates/llm-gateway-cli/src/config.rs:657-661), so a no-effort request to that model could reach the pod as high or xhigh and the acceptance does not say which"
- file: .engineering/planning/story/max-tokens-clamp.md
  line: 63
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "criterion 1 uses a window w without saying which window applies, and the no-clamp case for a hosted model that declares no context_window (stated in the ESS-first section) appears in no acceptance criterion, so a hosted model with no window can be clamped or counted and still pass"
- file: .engineering/planning/story/max-tokens-clamp.md
  line: 73
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "criterion 3 says a failed count \"is counted in the counters\" but names no counter, and the metrics are a fixed set of series (crates/llm-gateway/src/metrics.rs:4), so nobody can say which value to read before and after"
```
