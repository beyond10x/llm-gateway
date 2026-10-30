---
format: aep.planning-md/3
id: story:serve-cli-echoes-no-argument
kind: story
status: active
title: The serving command line never echoes an unexpected argument
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/binary.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:20Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:21Z", actor: "human:timo", revision: 4}
---
## Outcome

The serving command line refuses an unexpected argument without echoing it: `b10x-llm-gateway
--config d.toml PASTED-KEY-VALUE` exits 2 and stderr does not contain `PASTED-KEY-VALUE`.

## Why

`credential set` already suppresses clap's echo because the argument may be a key
(`crates/llm-gateway-cli/src/credential.rs:596-601`). The serving path uses clap's default error
printing (`crates/llm-gateway-cli/src/serve.rs:241`), which prints the argument. The existing test
`c1_a_subcommand_is_refused` in `crates/llm-gateway-cli/tests/binary.rs` expects the echo and changes
with this story.

Found by the adversarial review of 2026-10-30 (claim C2), proven by the test below failing on main
70841f2a.

## Acceptance

`review_c2_a_key_pasted_as_an_argument_is_not_echoed` in `crates/llm-gateway-cli/tests/binary.rs`
passes; `c1_a_subcommand_is_refused` is updated to the new message and passes; the generated CLI
reference (`llm-gateway-docs generate --check`) is current.

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/tests/binary.rs`, `CHANGELOG.md`.
