---
format: aep.planning-md/3
id: story:ingress-auth-off-warning-first
kind: story
status: active
title: The ingress-auth-off warning is printed before the listener binds
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/binary.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:21Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:21Z", actor: "human:timo", revision: 4}
---
## Outcome

With `ingress_auth = "none"`, the binary prints its `INGRESS_AUTH_OFF` warning before it binds the
listener, as `docs/gateway.md:252-254` says.

## Why

`start_with` binds and marks ready (`crates/llm-gateway-cli/src/relaying.rs:1185-1205`) before
`main_with` prints the warning (`crates/llm-gateway-cli/src/serve.rs:261-262`), so an operator who
reads the log sees the warning after the port already serves unauthenticated model calls.

Found by the adversarial review of 2026-10-30 (claim C4); the order was read from the code and
confirmed against the doc line.

## Acceptance

`the_ingress_auth_off_warning_is_written_before_the_listener_serves` in
`crates/llm-gateway-cli/tests/binary.rs` starts the binary with `ingress_auth = "none"` and its
stderr pipe held full, asserts `/ready` does not answer within 5 s, and after the pipe is drained
that the warning comes before the listening line. It fails on 70841f2a and passes after.

## Scope (inferred)

`crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/src/relaying.rs`,
`crates/llm-gateway-cli/tests/binary.rs`, `CHANGELOG.md`.
