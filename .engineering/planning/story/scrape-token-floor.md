---
format: aep.planning-md/3
id: story:scrape-token-floor
kind: story
status: active
title: A scrape bearer shorter than 32 bytes is refused at startup
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: cited
  path: website/docs/reference/deployment-document.md
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:21Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:22Z", actor: "human:timo", revision: 4}
---
## Outcome

The binary refuses at startup a `metrics_bearer_file` whose token is shorter than 32 bytes, naming
`metrics_bearer_file`, as it refuses a short owner secret.

## Why

On a cluster without NetworkPolicy enforcement the scrape bearer is the only guard on the
metrics-only listener (`website/docs/guides/run-on-kubernetes.md:271-274`). `metrics_bearer`
(`crates/llm-gateway-cli/src/serve.rs:93`) builds it with `OwnerToken::new`, whose check
(`crates/llm-gateway/src/auth.rs:52-62`) admits one byte, while the owner secret is refused under 32
bytes (`auth.rs:288-289`) "rather than serving a guessable one"
(`website/docs/concepts/single-owner-gateway.md:120`). Impact is read-only counters.

Found by the adversarial review of 2026-10-30 (claim M1), proven by the test below failing on main
70841f2a: a one-byte token started and served.

## Acceptance

`review_m1_a_one_byte_scrape_token_is_refused_at_startup` in `crates/llm-gateway-cli/tests/binary.rs`
passes; existing `metrics_bearer_file` tests use tokens of 32 bytes or more and pass;
`website/docs/reference/deployment-document.md` and the refusal reference state the floor.

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/tests/binary.rs`,
`website/docs/reference/deployment-document.md`, `CHANGELOG.md`.
