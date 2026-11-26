---
format: aep.planning-md/3
id: review-result:adversary-w05-finished-call-facts-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on finished-call-facts: needs change'
relations:
- reviews: story:finished-call-facts
revision: 1
---
## Verdict

NEEDS-CHANGE on unit U2 of wave 2026-11-26-w05 (story:finished-call-facts), reviewed at the wave
commit 2098a71a. Red cases on adv2/2026-11-26-w05 at 1e505f62: 2 of 2 fail.

## Findings

1. `crates/llm-gateway/src/server.rs:816`: an answer that carried its wire's terminal error and was
   then cut short by its upstream ends `UpstreamCut`, against the documented precedence (WireError
   first): `note_terminal_error` keeps an earlier `CutShort` (`relay.rs:2050`). Introduced.
2. `crates/llm-gateway/src/server.rs:575`: for a call refused at admission the value carries the
   real session hash while the dashboard is told the empty session, so acceptance 3 (equal hashes)
   and acceptance 7 (dashboard unchanged) cannot both hold for these calls. Introduced. Note.
3. `crates/llm-gateway/src/server.rs:819`: a translated call whose upstream fails after output is
   always `WireError` and its `upstream-failed` code is lost, so a translated call never reaches
   sessionspec as `upstream_cut`. Introduced. Note.
4. `spec/domains/telemetry.yaml:183`: an answer with a content coding is copied as the encoded
   bytes, which sessionspec's `Answer::Body` (a whole JSON body) cannot parse: a compressed
   passthrough answer lifts without its message id, usage and tool uses. Introduced.
5. `checks/conformance/src/gateway.rs:1231`: the conformance flake is the scenario
   `status-a-hosted-model-and-a-route-are-ready-when-their-endpoint-answers-and-unreachable-saying-why`:
   the fake TLS pod records `tls-refused` after the gateway already probed the next pod, so the
   `connections` fact is reordered (1 failure in 45 runs). Origin undecided; the scenario is older
   than the unit.

## Held

The `data:` reader matches the server-sent events rules (line ends, comments, multi-line data, an
event split at any point, an unended last event). Request and answer count together against the
bound, checked before each append, and only bytes written to the client are copied. Count-tokens,
shed, failed-auth and failed-review requests hand no value; there is one value per call. A client
leaving during a passthrough retry ends `ClientDisconnected`. A framing failure after admission is
`Refused` with an empty request copy. The dashboard takes its timing from the same value.
