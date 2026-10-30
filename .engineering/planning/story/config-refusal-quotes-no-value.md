---
format: aep.planning-md/3
id: story:config-refusal-quotes-no-value
kind: story
status: active
title: A deployment-document refusal never quotes the refused value
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/config.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:20Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:20Z", actor: "human:timo", revision: 4}
---
## Outcome

A deployment document refused for a value outside a closed set names the field and line and does not
quote the value: `auth_kind = "PASTED-KEY-VALUE"` is refused without `PASTED-KEY-VALUE` in the message.

## Why

The closed-enum deserializer's `unknown_variant` quotes the rejected value
(`crates/llm-gateway-cli/src/config.rs:42`), `schema_refusal` (`config.rs:682-698`) forwards it, and
`serve.rs:257` prints it to stderr. A key typed into the wrong field lands in the log.
`config.rs:123-139` and `website/docs/reference/deployment-document.md:146, 176-178` promise
value-free refusals.

Found by the adversarial review of 2026-10-30 (claim C1), proven by the test below failing on main
70841f2a: the refusal printed "unknown variant `PASTED-KEY-VALUE`".

## Acceptance

`review_c1_a_credential_typed_into_a_closed_enum_is_not_quoted_in_the_refusal` in
`crates/llm-gateway-cli/tests/config.rs` passes; every other test in that file still passes.

## Scope (inferred)

`crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/tests/config.rs`, `CHANGELOG.md`.
