---
format: aep.planning-md/3
id: review-result:client-access-acceptance-round-2
kind: review-result
status: active
title: Acceptance critic, epic:client-access, round 2
relations:
- reviews: epic:client-access
- reviews: story:model-tool-calling
- reviews: story:pod-proxy-tls
- reviews: story:client-qualification
- reviews: story:claude-code-wire
- reviews: story:loom-qualification
- reviews: story:public-model-listing
revision: 1
---
needs-revision

story:client-qualification — acceptance 5 reads the cold-start UTC times "from the binary's log lines", but the only story it depends on for them, `story:gateway-observability`, names no pod-start or readiness log event or timestamp in its acceptance. Its `UsageRecord` carries `duration_ms` and no time, and its O3 clause is only "row covered, with a test that names the row id". No depended story is shown to produce the line (part of the round-1 finding still holds; the request lines now come from the capture) — `.engineering/planning/story/client-qualification.md:58-59`, `.engineering/planning/story/gateway-observability.md:59-68`, `spec/domains/telemetry.yaml:59-84`
story:public-model-listing — criterion 10 says "The plain-text and HTML answers carry the Loom profile" but its only test "reads them out of the plain-text answer", so the HTML half has no check — `.engineering/planning/story/public-model-listing.md:100-103`
epic:client-access — Done when allows a story to be `archived` with a reason and also requires `docs/verification/` to hold the Loom record, which only `story:loom-qualification` produces, so an archived Loom story leaves the epic both done and not done — `.engineering/planning/epic/client-access.md:56-57`

The other nine round-1 findings are fixed:

- **Request and answer record:** `story:client-qualification` and `story:loom-qualification` now take the request lines from a loopback capture (client-qualification Method, `story:loom-qualification` acceptance 1).
- **Claude Code fixture:** `story:client-qualification` acceptance 3 commits it.
- **Tool-call outcome:** `story:client-qualification` acceptance 2 makes a session with no completed tool call a failure.
- **Loom wire and alias:** `story:loom-qualification` acceptance 2 names `POST /v1/chat/completions`, the alias, the tool call and its result.
- **TLS declaration:** `story:pod-proxy-tls` acceptance 5 checks the `DECIDED` line.
- **Declaration reaching the relay:** `story:model-tool-calling` acceptance 5 is a process test covering a declared `Parsed` and `Absent` model.
- **Empty `tools` array:** `story:model-tool-calling` acceptance 3 is now three separate scenarios, so `"tools": []` is its own.
- **Client-profile gating:** `story:public-model-listing` criterion 11 has one test per condition.
- **Profile values and marker:** `story:public-model-listing` criteria 9 and 12 check the alias and `context_window` values and replace the `UNMAPPED` marker with a `DECIDED` line.

What I read: 7 of 7 ids. They are `epic:client-access`, `story:model-tool-calling`, `story:pod-proxy-tls`, `story:client-qualification`, `story:claude-code-wire`, `story:loom-qualification` and `story:public-model-listing` (including its `## Client profiles` section, criteria 9-12). I used `aep plan artifact show` on each, plus `review-result:client-access-acceptance-round-1`, `story:gateway-observability`, `story:gateway-deployment` and the three blockers. In the tree I read `spec/domains/clients.yaml`, `spec/domains/deployment.yaml`, `spec/domains/telemetry.yaml` and `docs/gateway.md`. I also grepped `checks/conformance/src/gateway.rs`, which has the `acquired` counter that `story:model-tool-calling` acceptance 2 needs. `crates/llm-gateway/tests/gateway.rs` has the test that provokes every `RefusalCode`, which acceptance 4 needs.

What I could not establish:
- Whether `docs/design/runpod-clients.md` was consulted in full; I relied on the specs and stories that cite it.
- Whether a second reader would read the Runpod console billing page the same way (`story:client-qualification` acceptance 6). This is an unease, not a finding.
- `story:claude-code-wire` acceptance 2 says `HEAD /api/hello` is "answered as an unauthenticated probe" without naming a status. A test pins whatever the code does. Round 1 passed it and I left it out of the findings. If a reader needs the status named, the fix is to state it there.
- Out of my lane, not counted in the verdict: if `story:gateway-observability` is meant to supply the pod-start and readiness lines, it should say so (design), and `story:public-model-listing` has no `depends_on` edge to `story:model-tool-calling` for criterion 11. Wait: the `story:public-model-listing` relations do now list `depends_on story:model-tool-calling`, so that second item is resolved.

```findings
- file: .engineering/planning/story/client-qualification.md
  line: 58
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 5 reads cold-start 'UTC times ... from the binary's log lines', but story:gateway-observability's acceptance (gateway-observability.md:59-68) names no pod-start or readiness log event or timestamp and UsageRecord (spec/domains/telemetry.yaml:59-84) carries duration_ms but no time, so no depended story is shown to produce the line"
- file: .engineering/planning/story/public-model-listing.md
  line: 100
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "criterion 10 says 'The plain-text and HTML answers carry the Loom profile' but its only test 'reads them out of the plain-text answer', so the HTML half has no check"
- file: .engineering/planning/epic/client-access.md
  line: 56
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: undecided
  message: "Done when lets a story be archived with a reason and also requires docs/verification/ to hold the Loom record, which only story:loom-qualification produces, so an archived Loom story leaves the epic both done and not done"
```
