---
format: aep.planning-md/3
id: review-result:gateway-features-scope-round-2
kind: review-result
status: active
title: Scope critic, epic:gateway-features, round 2
relations:
- reviews: epic:gateway-features
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: decision-blocker:usage-from-responses
revision: 1
---
needs-revision

story:usage-records — acceptance case 5 requires "the token series that `story:gateway-observability` exports", but no artifact creates token series, and the epic does not promise any. The epic's outcome is that the gateway "records what each model call cost in tokens" (the record, not a metric). Observability's outcome is llmgw's 13 counters (rows O1 and O2), none of which counts tokens. Either drop case 5, or have the body say the story adds the token series itself and name the series-name marker it settles — .engineering/planning/story/usage-records.md:53 (against .engineering/planning/epic/gateway-features.md:11 and .engineering/planning/story/gateway-observability.md "Outcome")

All other promises trace to an item or to an exclusion the epic names.

**What I read:** 6 artifacts, all in full: the epic, the 4 children and `story:gateway-observability`. I also read `spec/domains/telemetry.yaml`, the `HostedEndpoint` and `TargetAttempt` entries of `spec/domains/upstream.yaml`, and the matrix rows O1 to O3 and R4. Commands: `aep plan artifact show` on each id, `aep plan artifact graph`, `grep` and `sed`.

**Promises:** I extracted 6 and traced 6.

| Promise | Claimed by |
|---|---|
| Hosted endpoint serving | `story:hosted-endpoints` |
| Fallback within one request | `story:target-fallback` |
| Per-call token record | `story:usage-records`, with the non-parsed fields in `story:gateway-observability` "Per-call record" |
| The three specification nouns | the three children |
| Research-table rows for translation, metrics and lifecycle | `story:gateway-translation`, `story:gateway-observability`, `epic:hosting` |
| "Done when" | covers both stories, the blocker and `usage-records` under options A, B and C |

**Not in the set but not findings:**
- The record is split between `usage-records` and `observability` with no overlap, and that split is stated in both bodies.
- Archiving `usage-records` under option B or C is a named, honest omission recorded in "Done when".

**Could not establish:** I did not check whether `UsageRecord` should exist as a first-class output at all. That is a design question, which is outside my lane and does not set my verdict.

```findings
- file: .engineering/planning/story/usage-records.md
  line: 53
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance case 5 requires token series that no artifact creates: the epic promises a per-call record, not a metric, and story:gateway-observability's outcome is llmgw's 13 counters (rows O1, O2), none of which counts tokens"
```
