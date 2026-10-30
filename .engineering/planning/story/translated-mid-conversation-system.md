---
format: aep.planning-md/3
id: story:translated-mid-conversation-system
kind: story
status: implemented
title: A translated Messages request carries a mid-conversation system entry
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T06:33:16Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T06:33:16Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-30T06:44:24Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

A translated Messages request whose `messages` hold a `role: "system"` entry (Claude Code 2.1.300
sends `[user, system]`, the system entry a string holding its environment block) reaches a Bedrock
route and every other translated Messages route instead of `400 translation-unsupported`.

## Why

Reported 2026-10-30 by a consumer session running `claude -p` headless through a 0.16.0 gateway to
a Bedrock route: every run is refused. The same request without the system entry answers 200.
The refusal is llm's, not the gateway's: llm 0.18.0 `crates/llm-messages/src/codec.rs:543`
admits only `user` and `assistant` roles ("Messages role is outside the declared subset").
CHANGELOG 0.12.0 lists "a mid-conversation instruction" as still refused.

## Acceptance

- llm admits the entry and states where it goes (the system prompt, or a marked user block); the
  gateway moves to that llm release.
- A translation test sends Claude Code 2.1.300's recorded request shape (`[user, system]`, the
  system entry a string) to a Bedrock route and a messages-over-chat route and gets 200, the
  system text reaching the upstream where llm states.

## Depends on

llm widening its Messages ingress subset to a mid-conversation system entry (llm's own story).

## Decision

`b10x-chef-orchestrator`, 2026-10-30, D262: the llm fix goes alone, since the consumer is
blocked. Leading system entries join the system prompt; a later one stays in place as a marked
user text block. The llm agent sends this repository the verified tag; this story rides with the
pin move to it, and the consumer session is told the release.
