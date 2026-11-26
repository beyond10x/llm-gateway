---
format: aep.planning-md/3
id: story:finished-call-facts
kind: story
status: draft
title: One finished-call value per call, and a bounded copy of its bytes when declared, through one port
relations:
- decomposes: epic:interaction-records
- serves: vision:portable-model-inference
- depends_on: story:test-step-within-budget
revision: 4
---
## Outcome

Every model call that was given a call id, except a count-tokens call, hands the embedding one
finished-call value through one port, once: the call id, the session hash and the harness that
named it, the path, the inbound wire, the route and upstream, attempts, queue wait, time to first
token, the declared-price cost, how the call ended, and its start and end instants. An embedding
that declares it also receives a bounded copy of the call's request body and of its answer as the
client received it.

## Why

These facts exist today in two places that do not meet: the usage record (`UsageRecords`, call id,
tokens, `cost_usd`) and the dashboard's finished call (`crates/llm-gateway/src/dashboard.rs`
486-511: attempts with their waits, `queue_wait_ms`, `ttft_ms`, the session). sessionspec's wire
lift takes them as one `GatewayFacts` value per call, with the request and the answer
(`WireCall { request, answer: Answer::Body | Answer::Events }`). The gateway buffers the request
body (at most 32 MiB, `crates/llm-gateway/src/relay.rs:45`) but relays the answer as it streams and
keeps none of it; the usage scanner reads the usage object only (`crates/llm-gateway/src/usage.rs:155`).

## Design

- One port, declared on the relay as `UsageRecords` is:
  `FinishedCalls::finished(&self, call: &FinishedCall, bytes: Option<&CallBytes>)`.
  `CallBytes` is `Copied { request, answer: Body(bytes) | Events(Vec<bytes>) }` or
  `OverBound { request_bytes, answer_bytes }`; `bytes` is `None` unless the relay was declared
  `with_call_bytes(<bound>)`.
- The value is built at the one point where the call ends, the point that now feeds
  `Dashboard::finish` and `UsageRecords::record` (`crates/llm-gateway/src/server.rs`); the
  dashboard's finished call is read from it, so attempts, queue wait and time to first token are
  gathered once. The session reader and `session_hash` move out of the dashboard module to be
  shared; the dashboard's behaviour and snapshot do not change.
- Two process series in the gateway crate, counted through a public method the embedding calls:
  `llmgw_interaction_records_dropped_total` (a call the embedding could not queue) and
  `llmgw_interaction_records_skipped_total` (a call whose bytes were over the bound).

## Acceptance

1. `spec/domains/telemetry.yaml` declares the finished-call value, `CallBytes`, the bound, and the
   two series, each with its rule, with a `BOUNDARY:` comment naming sessionspec's `GatewayFacts`
   and `WireCall`; the scenarios hold them; `task spec-diff` names only additions.
2. The port receives the value once per call that carries `x-llm-gateway-call` (relayed,
   translated, passthrough, refused), never for a count-tokens call, a shed request or one that
   failed authentication.
3. The session hash and harness come from the shared reader and function; a test holds the value's
   hash equal to the dashboard's for each header (`x-claude-code-session-id`, `session-id`,
   `session_id`, `x-opencode-session`, the user agent).
4. How the call ended is one of: complete, client disconnected, cut short by its upstream, ended in
   its wire's terminal error, refused (with the refusal code). The declared-price cost is present
   exactly when the usage record's `cost_usd` is.
5. Without `with_call_bytes` the port gets `None` and no copy is made (a test counts the
   allocations of the copy path: none). With it, a call within the bound gets `Copied`, one over it
   `OverBound` with both sizes, and the client's answer is unchanged byte for byte on every wire and
   path.
6. A streamed answer's copy is the ordered list of `data:` payloads as the client received them; a
   test splits each event of a recorded stream at every byte offset. A translated call's copy is
   the inbound wire's request and the answer as translated for the client. A refusal hands the
   request and no answer.
7. The dashboard's tests pass unchanged.
8. Tests: one new module `finished_calls` of `crates/llm-gateway/tests/main.rs`, per wire and per
   ending, against a fake upstream on loopback; each test under 5 s, and the step's time before and
   after is in the pull request.
9. `CHANGELOG.md` names the new public items under `### Added`, and under `### Changed` each public
   struct that gained a field, as a break for a caller that builds it by literal; the release
   that carries the story takes a minor version step.
