---
format: aep.planning-md/3
id: review-result:gateway-features-design-round-2
kind: review-result
status: active
title: Design critic, epic:gateway-features, round 2
relations:
- reviews: story:usage-records
- reviews: story:gateway-observability
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
revision: 1
---
needs-revision

story:usage-records — acceptance 5 names "the token series that `story:gateway-observability` exports", but that story exports only llmgw's 13 series (O1/O2, none of them a token counter) and the spec gives token series no owner, so one seam is claimed by neither body; the body should say this story adds the token series (the `depends_on story:gateway-observability` edge already orders it), or observability's body should name them — `.engineering/planning/story/usage-records.md:53`, `docs/llmgw-capability-matrix.md:161-162`, `spec/domains/telemetry.yaml:13-16`

story:gateway-observability — "Per-call record" says the counters of rows O1 and O2 are counted from one record per model call, but `UsageRecord` has no field behind most of them: O1's `pod_starts_total`, `pod_reaps_total`, `instruction_views_total`, `endpoint_invalidations_total` and `cold_start_wait_seconds_total`, and O2's `route_cold_start_holds_total`; the section should limit "counted from the record" to the series it can feed, and state that the hold series needs `story:cold-start-hold`, which is a missing `depends_on` edge, not just a "Neighbours" shared-file note — `.engineering/planning/story/gateway-observability.md:33`, `spec/domains/telemetry.yaml:59-82`

What I read: 6 artifacts in the set plus 4 context stories and the two spec domains, with `relations`, `graph` (walked all 61 edges, store-wide, no cycle) and `validate` (valid). I also read `relay.rs:70-130` and `serve.rs:80-125`. The chain `hosted-endpoints` → `target-fallback` → `usage-records` has a written reason on every edge (shared `RelayTargets` port, shared `relay::serve`, shared `config.rs`/`serve.rs`), so it is a recorded trade-off and not a finding. The `usage-records` → `observability` split of `UsageRecord` is ordered by an edge, and the owner of each field is stated in `telemetry.yaml:28-31`. I did not read any `review-result` body.

What I could not establish:
- Whether `target-fallback` truly needs `hosted-endpoints`. Its acceptance runs on loopback fixtures at positions 0 and 1, and the stated reason is a design choice, so I did not file it.
- Out of my lane (parallel safety): `gateway-observability`, `gateway-deployment` and `cold-start-hold` share `server.rs` and `relay.rs` with no edge between them. The "Neighbours" prose says to order them but records none.
- Out of my lane (acceptance): the `gateway-observability` acceptance still lists only R4/O1-O3 as covered rows, and does not name the record, observation or series-name settlement that the "Per-call record" section adds.

```findings
- file: .engineering/planning/story/usage-records.md
  line: 53
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 5 names token series that story:gateway-observability exports, but observability covers only llmgw's 13 series (none a token counter) and the specification gives token series no owner, so the body should say this story adds them (the depends_on edge already orders it) or observability's body should name them"
- file: .engineering/planning/story/gateway-observability.md
  line: 33
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Per-call record section says the counters of rows O1 and O2 are counted from one record per call, but UsageRecord carries no field for O1's pod, reap, instruction-view, invalidation and cold-start-wait series or for route_cold_start_holds_total; the section should limit the claim to the series the record feeds and record a depends_on edge to story:cold-start-hold for the hold series"
```
