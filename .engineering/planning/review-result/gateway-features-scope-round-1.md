---
format: aep.planning-md/3
id: review-result:gateway-features-scope-round-1
kind: review-result
status: active
title: Scope critic, epic:gateway-features, round 1
relations:
- reviews: epic:gateway-features
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: story:hosted-endpoints
revision: 1
---
needs-revision
story:target-fallback — the epic promises "falls back between a route's ordered targets within one request", but no item makes a deployed route list a second target: the deployment document gives each model one target per wire, `story:hosted-endpoints` acceptance 1 lists "that model's target" in the singular, and `story:target-fallback` line 53 credits hosted-endpoints with "the second kind of target a route can list". The fallback can only be exercised through fixtures. Either hosted-endpoints should claim a multi-target model declaration or target-fallback should say fixtures are the whole claim. — .engineering/planning/story/target-fallback.md:53; spec/domains/deployment.yaml:140
story:usage-records — acceptance 4 (a `Refused` record with its refusal code, no target contacted) claims a per-request disposition record. The epic promises only "what each model call cost in tokens" (epic line 15), and it assigns counters and logs of refused calls to `story:gateway-observability` (rows O1-O3, O2 `route_refusals_total`). The refused-call record belongs there or should be dropped. — .engineering/planning/story/usage-records.md:47

**What you read.** 5 artifacts, plus `story:gateway-observability` for overlap, the two spec domains, and `deployment.yaml:140`. Commands: `aep plan artifact show` on the epic, the three stories, the blocker and gateway-observability; `aep plan artifact graph`; `aep plan artifact list`. Epic promises extracted: 3 outcome promises plus 1 sub-promise (upstream credentials from a file). 4 of 4 traced to an item, and 1 of those (fallback between ordered targets) is covered only at the library level (finding 1).

**What I could not establish.**
- Whether the embedding's target source can already hand out multi-target routes. I saw `RouteSummary` allow up to 64 targets, but I did not trace who builds them outside the binary.
- Out of my lane, did not set the verdict: `story:usage-records` does not cover `UpstreamFailed` records although the entity allows them (acceptance lane). `story:usage-records` depends on `story:gateway-observability`, which is in a different epic (design lane).
- Honest omissions left alone: the blocker's options B and C would drop the epic's third promise, but the drafter named this and the epic's Not-in-scope table does not exclude it.
- No exclusion in the Not-in-scope table is violated by any item.

```findings
- file: .engineering/planning/story/target-fallback.md
  line: 53
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the epic promises fallback between a route's ordered targets, yet no item lets a deployed route list a second target: the deployment document gives one target per model and hosted-endpoints lists 'that model's target', so the fallback is reachable only through fixtures and the body does not say so"
- file: .engineering/planning/story/usage-records.md
  line: 47
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 4 records a Refused call with its refusal code, which the epic does not ask for (it promises tokens per model call) and which overlaps story:gateway-observability's refusal counters (O2)"
```
