---
format: aep.planning-md/3
id: review-result:critic-scope-w08-round-1
kind: review-result
status: active
title: Scope critic, wave 2026-10-15-w01 new stories, round 1
relations:
- reviews: story:effort-default
- reviews: story:max-tokens-clamp
- reviews: epic:client-access
revision: 1
---
needs-revision

story:effort-default — its outcome and criterion 3 claim the effort mapping (`high`/`max` to `xhigh`, `low`/`medium`/`xhigh` unchanged), which `story:claude-code-wire` criterion 4 already claims and pins with a `Relay` scenario and a `wire_relay` test, so both stories would be marked done for one outcome. Narrow the outcome to the declared default and say the mapping is relied on from `story:claude-code-wire` — .engineering/planning/story/effort-default.md:39 (criterion 3 at :74); overlaps .engineering/planning/story/claude-code-wire.md:105
epic:client-access — the "Done when" counts "Each of the five stories" and the table lists five, so the epic reads as done while `story:effort-default` and `story:max-tokens-clamp`, which both decompose it (`aep plan artifact graph`), are unimplemented. Add both to the table and make "Done when" cover them — .engineering/planning/epic/client-access.md:56

**What you read:** 4 artifacts (`epic:client-access`, `story:effort-default`, `story:max-tokens-clamp`, `story:claude-code-wire`) with `aep plan artifact show`, plus `aep plan artifact graph`, `git grep` over spec/crates/contracts, and the critic rubric. I did not run `kinds` or `relations`. The `decomposes` edges to `epic:client-access` are visible in the graph output.

**Promises extracted from the consumer report: 3, traced to an item: 3.**
- Map `low`/`medium`/`high`/`max` to values vLLM accepts, or pass a thinking budget. Claimed by `story:effort-default` and also by `story:claude-code-wire` (the duplicate in the first finding).
- Effort dropped, so vLLM's server default applies and the first call thought for the full 32k. Claimed by `story:effort-default`'s declared-default insertion.
- Clamp `max_tokens` to the context window minus prompt tokens. Claimed by `story:max-tokens-clamp`.

I found no unclaimed promise. Not covered: the "thinking budget" alternative, which the report offered as an "or" and the drafter chose not to take.

**What I could not establish:**
- **Reach beyond the report, unsure.** Insertion of a declared effort in `story:effort-default` also applies to Runpod models, which already carry `reasoning_effort` as the pod's server default. The consumer's test used a static upstream. I treated it as traceable and not a finding.
- **Reach beyond the report, unsure.** `story:max-tokens-clamp` adds a new public refusal (`ContextExceeded`, 400), a `count_tokens` call, and counters. The refusal is the necessary case when `w - n < 1`; the counters are the least traceable part. I did not flag either.
- **Narrowing, recorded.** The clamp covers the messages wire only. The report names Claude Code and the story states this, so it is a recorded narrowing and not a silent one.
- **Out of my lane, not counted in the verdict.** Acceptance and design for the premise "0.7.0 drops effort" are the other critics'. `story:claude-code-wire` records it as unknown whether Claude Code sends effort at all (record item 7), and the existing relay already maps `high` (W4). `story:effort-default`'s Why therefore rests on the consumer's observation, not on a gateway fault.

```findings
- file: .engineering/planning/story/effort-default.md
  line: 39
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the outcome and criterion 3 claim the effort mapping (high/max to xhigh, low/medium/xhigh unchanged) that story:claude-code-wire criterion 4 already claims and pins, so two items claim one outcome; narrow the outcome to the declared-default insertion and state the mapping as relied on from story:claude-code-wire"
- file: .engineering/planning/epic/client-access.md
  line: 56
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "Done when counts 'Each of the five stories' and the table lists five, so the epic can be done while story:effort-default and story:max-tokens-clamp, which decompose it, are unimplemented; add both to the table and to Done when"
```
