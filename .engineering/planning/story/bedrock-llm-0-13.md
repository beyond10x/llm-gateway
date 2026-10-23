---
format: aep.planning-md/3
id: story:bedrock-llm-0-13
kind: story
status: active
title: 'Bedrock models on llm 0.13.0: llm''s pre-send check and carried anthropic-beta names'
summary: llm 0.13.0 pinned; prepare_bedrock uses llm_bedrock::check_request; an InvokeModel model's anthropic_beta allowlist carries the client's beta names
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: contracts/gateway/scenarios
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/bedrock.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/routes.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/bedrock.rs
- confidence: inferred
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/src/translate.rs
- confidence: inferred
  path: crates/llm-runpod/Cargo.toml
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: website/docs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T02:54:04Z", actor: "human:timo", revision: 3, executor: "agent:llm-gateway-session", correlation: "wave-2026-10-23-w02"}
- {from: "proposed", to: "active", at: "2026-10-23T02:54:04Z", actor: "human:timo", revision: 4, executor: "agent:llm-gateway-session", correlation: "wave-2026-10-23-w02"}
---
## Outcome

The gateway builds on llm 0.13.0. A Bedrock model's request is checked before anything is sent
with llm's own pre-send check, `llm_bedrock::check_request`, instead of rules the gateway copied
from llm 0.11.0. A Bedrock `InvokeModel` model may declare the Anthropic beta names it accepts
(`anthropic_beta = [...]`); a messages client's `anthropic-beta` names on that list reach
Bedrock as the Invoke body's `anthropic_beta`, in the client's order, and every other name is
dropped and logged by name.

## Why

llm 0.13.0 publishes `check_request`, which answers exactly what its client encodes for both
Bedrock operations, so the gateway's mirror of llm 0.11.0's Converse rules can go. It also
carries a turn's betas (`Hints::anthropic_beta`) and sends them on `InvokeModel`. Bedrock refuses
a beta name it does not support, and Claude Code sends several of its own, so carrying every
client name would fail its turns: the deployment names the betas a model takes.

## The rule

- **Dependencies.** Every llm dependency moves from tag 0.11.0 to 0.13.0; what 0.12.0's
  `Routes::resolvers` change and 0.13.0's `Hints::anthropic_beta` and `Advisory::AnthropicBeta`
  break is fixed.
- **Pre-send check.** `prepare_bedrock` refuses what `llm_bedrock::check_request(&request,
  &target)` refuses, for `Converse` and `AnthropicInvoke`; no refusal becomes an acceptance.
- **Declaration.** `anthropic_beta` on `[hosted.<alias>] kind = "bedrock"`: a list of names, each
  `llm_core::anthropic_beta_name`, distinct, at most `llm_core::MAX_ANTHROPIC_BETAS`, default
  empty. It is `config:value` naming `hosted.<alias>.anthropic_beta` on an endpoint, on a
  `converse` model, and for a name outside the rules.
- **Gateway library.** `TranslationRequest` carries the client's `anthropic-beta` header values
  instead of a flag; the translator decides, and no header is forwarded verbatim.
- **Carrying.** For an `InvokeModel` model and a messages client the request is decoded with
  `llm_messages::decode_request_with_headers`; the turn keeps only the allowlisted names, in the
  client's order. Each name dropped is logged with `field = "anthropic-beta"` and the name. A
  header the decoder refuses refuses the request with the code a bad body gets.
- **Every other upstream** (Converse, chat, responses, a non-Bedrock messages endpoint) carries no
  beta, as before.

## Acceptance

1. Against the fake Bedrock: allowlisted names reach the Invoke body as `anthropic_beta`, in the
   client's order; a name off the list is absent from it and logged by name; with no list nothing
   is carried and the header is logged as dropped; a Converse model never carries one; a
   malformed `anthropic-beta` header is refused before Bedrock is reached.
2. The `anthropic_beta` refusals above are `config:value` naming the key.
3. Every Bedrock test that passed on llm 0.11.0 passes on 0.13.0 with `check_request` in place of
   the mirrored rules, and the mirror is gone.
4. `ess specify validate --path spec` passes with the key in `HostedDeclaration`; the suite and
   schema are regenerated with ess 0.56.0.
5. `docs/gateway.md`, the site, README, AGENTS.md and CHANGELOG describe the key and the 0.13.0
   pin; no Bedrock beta name is claimed verified against AWS.
