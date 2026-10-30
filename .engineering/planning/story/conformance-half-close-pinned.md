---
format: aep.planning-md/3
id: story:conformance-half-close-pinned
kind: story
status: active
title: A test fails when the conformance client half-closes by default
relations:
- serves: vision:portable-model-inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T07:21:28Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T07:21:29Z", actor: "human:timo", revision: 3}
---
## Outcome

A test fails when the conformance client half-closes its connection by default: a relay
scenario whose target answers after the relay's 100 ms client watch passes with the default
(the client keeps its sending side open) and fails when the default half-closes.

## Why

`review-result:adversary-w01-relay-hosting-pass-1` finding 3: the conformance client
half-closed by default, which the relay treats as a gone client, so relay scenarios passed only
by answering inside the 100 ms watch. Wave w01 changed the default to `false`
(`checks/conformance/src/gateway.rs`, `half_close.unwrap_or(false)`), but no test fails when
the default goes back to `true`.

## Acceptance

- A test or conformance scenario with a target that answers after more than 100 ms passes on
  the current code and fails with `half_close.unwrap_or(true)`; both runs are recorded.
