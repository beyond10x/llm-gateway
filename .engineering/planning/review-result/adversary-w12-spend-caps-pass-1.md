---
format: aep.planning-md/3
id: review-result:adversary-w12-spend-caps-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:spend-caps, pass 1
relations:
- reviews: story:spend-caps
revision: 1
---
unit: story:spend-caps, adversary pass 1, covering adv/spend-caps at ea1b836 (impl/spend-caps 28e4787 plus one test commit)
verdict: red (one blocker, one warning, two notes; all introduced)
cases: executed 374→378, red 3
origin: n/a (adversary: introduced 4, pre-existing 0, undecided 0)
wrote-outside-worktree: run script, run log and a removed mutant clone in the wave scratch directory

1. Diff since 28e4787: two new adversary test files (cli, runpod) and 21 lines appended to crates/llm-runpod/tests/transport.rs; tests only.
2. Red, each case alone: the shipped composition sends a create with a ceiling declared and never reads the account; two starts in a row are both admitted against one headroom; a reported 4.03 USD reads as 4030001 micro-USD.
3. Mutant: asking for spend from now instead of the start of the UTC day keeps conformance at 259/259; the new day-start case catches it.
4. Attacked and not broken: overflow (u128 saturation, checked_add), a clock going backwards, daily before total, equality at one micro-USD, malformed reads all unknown, a restored pool, config edges 1e-6 and 1e6, two providers with ceilings refused, the counter apart from start failures.
5. Not attacked: a slow spend read past the startup deadline.
6. Decisions (coordinator): finding 2, a live pod counts at least its startup estimate; finding 3, reported amounts and declared values round to the nearest micro-USD, computed estimates round up; finding 4, the emulator answers spend from timed entries at or after the asked day start, with a scenario across the day boundary.

```findings
[{"file":"crates/llm-gateway-cli/src/relaying.rs","line":855,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"start_connected, the shipped binary's composition, builds the pool without with_spend, so no ceiling is ever read or enforced"},
 {"file":"crates/llm-runpod/src/provider.rs","line":150,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"a live pod submitted moments ago adds a 0 running estimate, so two starts in a row are both admitted against one headroom"},
 {"file":"crates/llm-runpod/src/spend.rs","line":136,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"ceil(usd*1e6) reads ordinary cent amounts one micro-USD high, refusing a start that lands exactly on the ceiling"},
 {"file":"crates/llm-runpod/src/emulated.rs","line":285,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the emulator ignores day_start_ms, so conformance stays green when spend is asked from now instead of the start of the UTC day"}]
```
