---
format: aep.planning-md/3
id: story:translation-refusal-names-path
kind: story
status: draft
title: A translation refusal names the refused path in a log line
relations:
- decomposes: epic:gateway
revision: 2
---
## Outcome

A `translation-unsupported` or `translation-invalid` refusal names what llm refused, as a path
or field name and never a value, in an info log line, so an operator can find the refused field
without a recording proxy.

## Why

Reported 2026-10-30 by a consumer session: a Claude Code request was refused field by field
(`tools[].defer_loading`, `thinking.display`, `context_management`, `safeguards`, a system-role
message) and the gateway named none of them, even at `RUST_LOG=debug`. `refusal()` in
`crates/llm-gateway-cli/src/translate.rs` maps llm's error to a code and drops its message.

## Acceptance

- Each refusal llm's decoders return for an unsupported or invalid field writes one info line
  naming the refused path, proved by a test per ingress wire.
- llm's message texts are audited first: a text that embeds a client value is not logged, and a
  test sends a value with a marker and finds it in no log line.

## Decision

`b10x-chef-orchestrator`, 2026-10-30, D262: this story goes in the next llm-gateway wave after
0.18.0.
