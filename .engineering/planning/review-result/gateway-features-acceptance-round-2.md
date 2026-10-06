---
format: aep.planning-md/3
id: review-result:gateway-features-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, epic:gateway-features, round 2
relations:
- reviews: epic:gateway-features
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: decision-blocker:usage-from-responses
- reviews: story:gateway-observability
- reviews: story:ess-0-53
revision: 1
---
needs-revision
story:gateway-observability — the "Per-call record" section commits the story to filling the `UsageRecord` fields, adding the conformance observation and settling the series-name and record-destination markers, but the Acceptance still reads "Each named row is `covered` in the matrix…", so none of those deliverables can be checked, and `story:usage-records` builds on them — .engineering/planning/story/gateway-observability.md:23
story:usage-records — case 5 reads "the token series that `story:gateway-observability` exports", but that story's scope is llmgw's 13 counters, none of which counts tokens (rows O1, O2), and the series names are still `UNMAPPED`, so the series case 5 checks has no owner — .engineering/planning/story/usage-records.md:53
story:usage-records — the Depends section says one record per call "has to account for the attempt loop", yet no case has a call that falls back, so whose counters the record carries after a failed first attempt cannot be observed — .engineering/planning/story/usage-records.md:60
story:hosted-endpoints — the Acceptance does not say what runs each case, and cases 5 and 6 are startup refusals, which neither command (`Exchange`, `Relay`) observes and which this repo proves with crate tests — .engineering/planning/story/hosted-endpoints.md:33
epic:gateway-features — "Done when" requires `story:hosted-endpoints` acceptance cases to pass "as `Relay` conformance scenarios", but its cases 1, 5 and 6 are an inspection response and two startup refusals, so the epic's check cannot be met as written — .engineering/planning/epic/gateway-features.md:78
story:ess-0-53 — "or each difference is explained" names no place or reader for the explanation, so any scenario-total difference passes — .engineering/planning/story/ess-0-53.md:31

What I read: 7 of 7 ids via `aep plan artifact show` (epic:gateway-features, story:hosted-endpoints, story:target-fallback, story:usage-records, decision-blocker:usage-from-responses, story:gateway-observability, story:ess-0-53). I also read `aep plan artifact kinds` and `lifecycle` for epic, story and decision-blocker, `spec/domains/upstream.yaml`, `spec/domains/telemetry.yaml`, `spec/domains/deployment.yaml` and `spec/domains/gateway.yaml` (command list only), and matrix rows R4, O1-O3, W7, B8, B9 and K28. I did not open any `review-result`. `story:target-fallback` and `decision-blocker:usage-from-responses` held up: each case there is observable and the blocker's "Cleared when" names the transition.

What I could not establish:
- Whether case 4 of `story:hosted-endpoints` (an untrusted certificate on a loopback TLS fixture) is feasible in the conformance harness. That is a design question, outside my lane.
- I did not run `ess specify validate`, so the epic's claim that the planned domains validate on 0.52.0 and 0.53.0 is unchecked.
- The `<source>` name in case 5 of `story:hosted-endpoints` is unspecified. I treated that as the drafter's own open marker, not a finding.

Out of my lane, not counted toward the verdict:
- The `Neighbours` and `Ordering` sections and the `depends_on` edges are parallel-safety and design matters.
- The `Scope (inferred)` lists are scope matters.

```findings
- file: .engineering/planning/story/gateway-observability.md
  line: 23
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Per-call record section commits the story to filling the UsageRecord fields, adding the conformance observation and settling the series-name and record-destination markers, but the Acceptance still reads 'Each named row is covered in the matrix…', so none of those deliverables can be checked, and story:usage-records builds on them"
- file: .engineering/planning/story/usage-records.md
  line: 53
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "case 5 reads 'the token series that story:gateway-observability exports', but that story's scope is llmgw's 13 counters, none of which counts tokens (rows O1, O2), and the series names are still UNMAPPED, so the series case 5 checks has no owner"
- file: .engineering/planning/story/usage-records.md
  line: 60
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Depends section says one record per call 'has to account for the attempt loop', yet no case has a call that falls back, so whose counters the record carries after a failed first attempt cannot be observed"
- file: .engineering/planning/story/hosted-endpoints.md
  line: 33
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Acceptance does not say what runs each case, and cases 5 and 6 are startup refusals, which neither command (Exchange, Relay) observes and which this repo proves with crate tests"
- file: .engineering/planning/epic/gateway-features.md
  line: 78
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "Done when requires story:hosted-endpoints acceptance cases to pass 'as Relay conformance scenarios', but its cases 1, 5 and 6 are an inspection response and two startup refusals, so the epic's check cannot be met as written"
- file: .engineering/planning/story/ess-0-53.md
  line: 31
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'or each difference is explained' names no place or reader for the explanation, so any scenario-total difference passes"
```
