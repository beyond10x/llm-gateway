---
format: aep.planning-md/3
id: review-result:client-access-acceptance-round-1
kind: review-result
status: active
title: Acceptance critic, epic:client-access, round 1
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

story:client-qualification — acceptance 1 and 2 take "the request lines the gateway received" and the cold-start time "read from the gateway's standard error with UTC times", but the binary writes only the `listening`, `refused` and `stopped` lines, and no story it depends on adds request or start lines, so neither record can be produced — `.engineering/planning/story/client-qualification.md:35-39`; `spec/domains/deployment.yaml:14-18`
story:client-qualification — acceptance 1 records request lines and non-2xx bodies but no Claude Code inference request body, while `story:claude-code-wire` acceptance 1 needs "a request body Claude Code 2.1.293 sent, captured by `story:client-qualification`" as a fixture, so nothing here yields it — `.engineering/planning/story/client-qualification.md:35` and `.engineering/planning/story/claude-code-wire.md:54`
story:client-qualification — the Outcome says each session "completes a task that makes at least one tool call", but acceptance 1 only records "whether a tool call completed" and acceptance 4 lets any failure be an open finding, so a record saying no tool call completed still closes the story — `.engineering/planning/story/client-qualification.md:21`, `:36-37`
story:loom-qualification — acceptance 1 takes "the request lines the gateway received" from a binary that logs none (same gap as `story:client-qualification`) — `.engineering/planning/story/loom-qualification.md:31-33`; `spec/domains/deployment.yaml:14-18`
story:loom-qualification — the Outcome says the run completes a turn with a tool call, on the `chat` wire, against the same model as the other two sessions, but acceptance 1 records only "whether the tool call completed" and names no wire or model, so a failed run or a run on another model or wire satisfies it — `.engineering/planning/story/loom-qualification.md:18`, `:31-33`
story:pod-proxy-tls — "ESS first" says `spec/domains/deployment.yaml` declares the verified-certificate rule and the trust roots, but no acceptance item checks that declaration, so it can be skipped while all four items pass — `.engineering/planning/story/pod-proxy-tls.md:45-47`, `:50-60`
story:model-tool-calling — the Outcome says a model "in the deployment document declares" tool calling, but acceptance 2 and 3 are `Relay` conformance scenarios and 5 covers only the omitted key, so nothing checks that a declared value in the document reaches the relay (`config.rs` to `serve.rs`) — `.engineering/planning/story/model-tool-calling.md:40-44`, `:62-74`
story:model-tool-calling — acceptance 3 says "Two `Relay` scenarios" and then lists a `Parsed` request plus an `Absent` request "with no `tools` key, or `"tools": []`", so it does not say whether the empty-array case is a scenario of its own — `.engineering/planning/story/model-tool-calling.md:68-69`
story:public-model-listing — the `## Client profiles` section is prose after the numbered acceptance. It says a profile is "offered only for a model that declares its wire and whose tool calling is `Parsed`", but no item checks that a model that fails either condition gets none — `.engineering/planning/story/public-model-listing.md:96-98`
story:public-model-listing — the section's one test "reads each setting name of the profile out of the answer", so the "alias and `context_window` filled in" has no check, and no item says the `UNMAPPED` marker on `ClientProfile` is replaced by a `DECIDED` line — `.engineering/planning/story/public-model-listing.md:94-99`; `spec/domains/clients.yaml:106`

What I read: all 7 ids (`epic:client-access`, `story:model-tool-calling`, `story:pod-proxy-tls`, `story:client-qualification`, `story:claude-code-wire`, `story:loom-qualification`, `story:public-model-listing`), whole bodies via `aep plan artifact show`. I also ran `aep plan artifact kinds` and `lifecycle story` / `lifecycle epic`, and read `spec/domains/clients.yaml`, `docs/design/runpod-clients.md` §1, §5 and §6, `docs/gateway.md`, `spec/domains/deployment.yaml:10-45`, and `review-result:replan-acceptance-round-2` for the standard applied before.

What I could not establish:
- Whether the Runpod console billing page (`story:client-qualification` acceptance 3) is something a second person can read the same way. This is an unease, not a finding.
- No finding on `story:claude-code-wire` or `epic:client-access` bodies: their criteria are checkable, and the `docs/gateway.md` transition holds (it says nothing about `anthropic-*` headers today).
- Out of my lane, not counted in the verdict:
  - `story:public-model-listing`'s Client profiles section needs `Parsed` from `story:model-tool-calling` but has no `depends_on` on it (design).
  - `story:client-qualification` and `story:loom-qualification` need request and start logging that `story:gateway-observability` (O3) may supply, and neither depends on it (design).

```findings
- file: .engineering/planning/story/client-qualification.md
  line: 35
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 and 2 take 'the request lines the gateway received' and cold-start times 'read from the gateway's standard error with UTC times', but the binary writes only listening, refused and stopped lines (spec/domains/deployment.yaml:14-18) and no depended story adds request or start lines, so the records cannot be produced"
- file: .engineering/planning/story/client-qualification.md
  line: 35
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 records request lines and non-2xx bodies but no Claude Code inference request body, which story:claude-code-wire acceptance 1 (claude-code-wire.md:54) consumes as a fixture 'captured by story:client-qualification'"
- file: .engineering/planning/story/client-qualification.md
  line: 21
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome says each session 'completes a task that makes at least one tool call' but acceptance 1 only records 'whether a tool call completed' and acceptance 4 lets any failure be an open finding, so a record saying no tool call completed still closes the story"
- file: .engineering/planning/story/loom-qualification.md
  line: 31
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 takes 'the request lines the gateway received' from a binary that writes none (spec/domains/deployment.yaml:14-18)"
- file: .engineering/planning/story/loom-qualification.md
  line: 18
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome says the run completes a turn with a tool call on the chat wire against the same model as the other sessions, but acceptance 1 records only 'whether the tool call completed' and names no wire or model, so a failed run or a run on another model or wire satisfies it"
- file: .engineering/planning/story/pod-proxy-tls.md
  line: 45
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "'ESS first' says spec/domains/deployment.yaml declares the verified-certificate rule and the trust roots, but no acceptance item checks that declaration"
- file: .engineering/planning/story/model-tool-calling.md
  line: 40
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Outcome says a model in the deployment document declares tool calling, but acceptance 2 and 3 are Relay conformance scenarios and acceptance 5 covers only the omitted key, so nothing checks that a declared value in the document reaches the relay"
- file: .engineering/planning/story/model-tool-calling.md
  line: 68
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 3 says 'Two Relay scenarios' and lists a Parsed request plus an Absent request 'with no tools key, or tools: []', so it does not say whether the empty-array case is a scenario of its own"
- file: .engineering/planning/story/public-model-listing.md
  line: 96
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Client profiles section is prose after the numbered acceptance and says a profile is offered only for a model declaring its wire and tool calling Parsed, but no item checks that a model failing either condition gets none"
- file: .engineering/planning/story/public-model-listing.md
  line: 94
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the section's one test reads each setting name out of the answer, so 'alias and context_window filled in' has no check, and no item says the UNMAPPED marker on ClientProfile (spec/domains/clients.yaml:106) is replaced by a DECIDED line"
```
