---
format: aep.planning-md/3
id: review-result:gateway-features-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, epic:gateway-features, round 1
relations:
- reviews: epic:gateway-features
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: decision-blocker:usage-from-responses
revision: 1
---
needs-revision

epic:gateway-features — no done-when section: the Outcome joins three independent results ("serves a model from a hosted endpoint … falls back … records what each model call cost") and nothing says when the epic counts as done, though the sibling `epic:gateway` has "Done when" — .engineering/planning/epic/gateway-features.md:11
story:hosted-endpoints — case 2 requires the request to carry "the endpoint's credential" but names no header for the `api-key` kind (the `auth_kind` values are anonymous, bearer, api-key), so two implementations can both pass — .engineering/planning/story/hosted-endpoints.md:40
story:target-fallback — the Outcome promises that the gateway "reports that target failed" and that two markers (a 429, per-attempt dispatch evidence) are settled before code, but no case observes the invalidation report, a 429, or the evidence — .engineering/planning/story/target-fallback.md:13
story:target-fallback — case 6 says the second body differs only in "the rewritten `model` value" but never requires that value to be position 1's own upstream name, which the Outcome states — .engineering/planning/story/target-fallback.md:47
story:usage-records — the Outcome's second sentence ("The counters are added to the series that `story:gateway-observability` exports") has no case, and the series names and the record's destination are both `UNMAPPED` in the telemetry spec, so "yields a record" has no stated observation point — .engineering/planning/story/usage-records.md:17
story:usage-records — "Every model call … produces one usage record" is covered for relayed and refused calls only; no case covers a call that ends `UpstreamFailed` (502), and none checks `reported_model`, `response_bytes` or `duration_ms` — .engineering/planning/story/usage-records.md:47
decision-blocker:usage-from-responses — the body says nothing on what clears the blocker (which option is chosen, by whom, and where the choice is recorded), so "cleared" can be asserted but not checked — .engineering/planning/decision-blocker/usage-from-responses.md:11

What I read: 5 of 5 ids given, via `aep plan artifact show <id>` for each. I also read `aep plan artifact lifecycle story` and `lifecycle decision-blocker`, `story:gateway-observability`, `epic:gateway`, `spec/domains/upstream.yaml`, `spec/domains/telemetry.yaml`, and the `Relay` scenario format in `contracts/gateway/scenarios/`.

What I could not establish:
- Whether an epic needs an acceptance section in this store. I judged it by the sibling `epic:gateway`, which has "Done when".
- Whether `api-key` is meant to use a fixed header. No source in `crates/` or `docs/gateway.md` names one.
- Out of my lane (design): `story:usage-records` depends on `story:gateway-observability`, which still has no series names. The one-record-per-call rule and fallback's multiple attempts also interact. Neither set my verdict.
- Out of my lane (scope): `story:gateway-observability` Outcome and Acceptance cover only the llmgw rows. It declares no usage counters for `story:usage-records` to feed.

```findings
- file: .engineering/planning/epic/gateway-features.md
  line: 11
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "no done-when section: the Outcome joins three independent results and nothing says when the epic counts as done, though the sibling epic:gateway has a Done when"
- file: .engineering/planning/story/hosted-endpoints.md
  line: 40
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "case 2 requires the request to carry the endpoint's credential but names no header for the api-key auth_kind, so two implementations can both pass"
- file: .engineering/planning/story/target-fallback.md
  line: 13
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome promises the gateway reports the failed target and the story settles the 429 and dispatch-evidence markers before code, but no case observes the invalidation report, a 429, or the evidence"
- file: .engineering/planning/story/target-fallback.md
  line: 47
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "case 6 says the second body differs only in the rewritten model value but does not require that value to be position 1's own upstream name"
- file: .engineering/planning/story/usage-records.md
  line: 17
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome's sentence that the counters are added to the series gateway-observability exports has no case, and the series names and the record's destination are UNMAPPED, so 'yields a record' has no stated observation point"
- file: .engineering/planning/story/usage-records.md
  line: 47
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "every model call produces one record is covered for relayed and refused calls only; no case covers a call that ends UpstreamFailed and none checks reported_model, response_bytes or duration_ms"
- file: .engineering/planning/decision-blocker/usage-from-responses.md
  line: 11
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the body says nothing on what clears the blocker (which option is chosen, by whom, where the choice is recorded), so cleared can be asserted but not checked"
```
