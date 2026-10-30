---
format: aep.planning-md/3
id: story:client-profiles-translated
kind: story
status: active
title: GET / offers each client only models its requests reach, translated or native
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:llm-0-7-0-pin
scope:
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: inferred
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway/src/listing.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/src/translate.rs
- confidence: inferred
  path: crates/llm-gateway/tests/public_listing.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: spec/domains/clients.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T10:09:03Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-17T10:09:03Z", actor: "human:timo", revision: 6}
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

## Review finding, 2026-10-30

The adversarial review of 2026-10-30 (claim M3) found that `GET /` hands a translated-only model
(a Bedrock model declaring only the messages wire) a Codex profile and a chat/Loom profile:
`profiled()` returns true for any wire when `translated_only()`
(`crates/llm-gateway/src/listing.rs:127-130`). `docs/gateway.md:166-168` and
`website/docs/guides/translate-between-wires.md:200-201` say a profile is given only on a wire the
model declares. `review_m3_a_translated_only_messages_model_gets_no_codex_or_loom_profile` in
`crates/llm-gateway/tests/public_listing.rs` (verification tree `llm-gateway-verify-2026-10-30`)
fails on main 70841f2a.

This story decides which translated models get a profile, so it owns the fix; the test's assertion
follows that decision and is not landed on its own.
