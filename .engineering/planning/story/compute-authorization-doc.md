---
format: aep.planning-md/3
id: story:compute-authorization-doc
kind: story
status: implemented
title: The hosting contract describes the compute authorization the gateway passes
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: docs/hosting.md
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:24Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:24Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-30T04:37:00Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`docs/hosting.md` describes the compute authorization the gateway actually passes: a fixed
`b10x-llm-gateway` authorization with reservation `unmetered` or `spend-ceiling`, with the Runpod
spend check as the only cap, and no longer says Declare requires an `llm-cost` Reserve and Begin.

## Why

`docs/hosting.md:222-225` says Declare requires a reservation the caller must Reserve and Begin in
the `llm-cost` ledger first, making unauthorized provisioning unrepresentable. The code passes a
literal (`crates/llm-gateway-cli/src/relaying.rs:56-59, 183-192`), the controller checks only the
ledger name (`crates/llm-provision/src/controller.rs:370-372`), and `docs/hosting.md:482` already
records the literal as fixed. The two sentences contradict each other; the code is what ships.

Found by the adversarial review of 2026-10-30 (claim H3), confirmed by reading both doc lines and the
code.

## Acceptance

`docs/hosting.md` § Declare and § the gateway's authorization agree with each other and with
`relaying.rs:183-192`; the design-review record lists the row as resolved.

## Scope (inferred)

`docs/hosting.md`.
