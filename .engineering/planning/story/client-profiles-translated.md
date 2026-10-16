---
format: aep.planning-md/3
id: story:client-profiles-translated
kind: story
status: draft
title: GET / offers each client only models its requests reach, translated or native
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/listing.rs
revision: 2
---
## Outcome

`GET /` hands Codex and Claude Code a client profile only for a model they can actually use. Today
it counts a model's declared (native) wires only (`crates/llm-gateway/src/listing.rs:124`,
`profiled()`), which was right before translation; with routes and translation it can hand Claude
Code a model whose name says another client, and it never offers a translated model. Decide which
models each profile offers once llm's ingress subset admits the requests those clients send.

## Why

Found by `review-result:adversary-w11-routes-document-pass-1` finding 5 (pre-existing at
20f48f2c): on the routes example of the translation guide, Codex gets no profile and Claude Code
gets the Anthropic route. Wave 2026-10-16-w02 left the rule unchanged and documented it, because
llm 0.6.0's translated subset refuses the fields Codex and Claude Code send
(`crates/llm-gateway-cli/tests/translation.rs`), so offering a translated model would mislead.

## Acceptance

`GET /` with a Codex or Claude Code `user-agent` names only models that client's requests reach
without a `translation-*` refusal, proved by a scenario per client over a routes document with one
native and one translated model.

## Depends on

llm widening its Responses and Messages ingress subsets (llm's own stories); not this repository's.
