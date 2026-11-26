---
format: aep.planning-md/3
id: story:cli-test-port-race
kind: story
status: implemented
title: A binary test binds a port no other test can take first
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T02:02:41Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T02:02:41Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-26T10:04:16Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Outcome

`crates/llm-gateway-cli/tests/binary.rs::c2_config_flag_loads_a_closed_toml` passes on every run:
the binary under test binds a port no other test can take between the choice and the bind.

## Why

Measured on the build node, 2026-11-26, in a `cargo test --locked --no-fail-fast -p
b10x-llm-gateway-cli` run on the joined wave 2026-11-25-w03: the test failed once with
`refused listen:bind: 127.0.0.1:45837: Address already in use (os error 98)` and passed in each of
the four runs after it. The test picks a free port, releases it, and hands the number to the
binary, which binds it later; another test of the same run can take it in between.

## Acceptance

1. The test no longer chooses a port that it then releases: the binary binds port 0 and the test
   reads the port it was given, or the test holds the port until the binary has it.
2. Every other test of the crate that hands a chosen port to a process it starts is changed the
   same way, each named in the pull request.
3. The crate's tests pass in ten runs in a row on the build node, run one after another.
