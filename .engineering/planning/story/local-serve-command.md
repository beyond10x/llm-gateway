---
format: aep.planning-md/3
id: story:local-serve-command
kind: story
status: implemented
title: b10x-llm-gateway serve reads the operator's own document by default
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/cli.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/binary.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T16:18:54Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-04T16:18:54Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-04T16:18:54Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`b10x-llm-gateway serve` starts the gateway from `~/.config/llm-gateway/config.toml` (or
`$XDG_CONFIG_HOME/llm-gateway/config.toml`), and `serve --config <file>` from another document.
The serving form `b10x-llm-gateway --config <file>` and the `credential` command are unchanged.

## Acceptance

1. `serve` with no `--config` reads the default document; a missing one is refused
   `config:not-found`-style through the trusted-file reader, naming the setting, never the path.
2. `serve --config <file>` serves exactly what `--config <file>` serves.
3. Every serving command line of 0.22.0 still parses and serves (`tests/binary.rs` `c1_*`).
4. A local `serve` bound to a loopback address with `ingress_auth = "none"` answers `GET /metrics`
   and the dashboard without the owner secret; any other bind still requires it.
