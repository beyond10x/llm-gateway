---
format: aep.planning-md/3
id: story:cold-start-hold
kind: story
status: active
title: A request for a cold model waits for its pod within a hold budget
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
scope:
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: checks/conformance/src/runpod.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: inferred
  path: crates/llm-gateway/tests/wire_relay.rs
- confidence: cited
  path: crates/llm-runpod/src/config.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: inferred
  path: crates/llm-runpod/tests/runpod.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: docs/hosting.md
- confidence: inferred
  path: spec/domains/gateway.yaml
- confidence: inferred
  path: spec/domains/runpod.yaml
revision: 19
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T15:31:44Z", actor: "human:timo", revision: 18}
- {from: "proposed", to: "active", at: "2026-10-08T15:31:44Z", actor: "human:timo", revision: 19}
---
## Outcome

A request for a model whose pod is not serving is held within a request hold budget separate from the startup deadline, answered 503 with Retry-After past it; the reaper runs on a timer and orphans are swept at startup.

## Rows

L6, W6, K28, L13, L15, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

Each named row is `covered` in the matrix, with the llm-gateway citation updated and a test that
names the row id.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.

## Scope

Derived 2026-10-06 by `story-scoper` at `a11ef83`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-runpod/src/pool.rs` — cited: the matrix rows W6 (`:69-70`, `:458`), L6 (`:410-414`), L13 (`:462-490`) and L15 (`:340-350`) name it
- **Files:** `docs/llmgw-capability-matrix.md` — cited: the acceptance turns W6, K28, L6, L13 and L15 to `covered`, with their citations updated
- **Files:** `crates/llm-runpod/src/config.rs` — cited: K28 names `startup_deadline_ms` (`:76-77`), and the hold budget is a separate setting beside it
- **Symbols:** `RunpodPool::ensure`, `RunpodPool::reap`, `RunpodPool::restore`, `PoolError::Starting`, `RunpodModel::startup_deadline_ms` — cited
- **Also likely (W6, the HTTP answer):** `crates/llm-gateway/src/relay.rs` — inferred: `RelayTargets::acquire` returns `Option` (`:89`) and `None` becomes `target-unavailable` (`:573-576`), so a separate cold-start answer needs that seam changed
- **Also likely:** `crates/llm-gateway/src/error.rs` — inferred: a new 503 refusal code next to `TargetUnavailable` (`:120`), kebab-case as W3 decided
- **Also likely:** `crates/llm-gateway/src/server.rs` — inferred: the refusal writer (`:382`) and the header assembly (`:629`) are where `Retry-After` would go
- **Also likely:** `docs/gateway.md` — inferred: its refusal table (`:186-200`) must match the code (AGENTS.md invariant)
- **Also likely:** `crates/llm-gateway/tests/gateway.rs` — inferred: it triggers every `RefusalCode` (`:491`, `:558`) and `include_str!`s `docs/gateway.md`
- **Also likely:** `crates/llm-gateway/tests/wire_relay.rs` — inferred: W rows are closed by tests here named with the row id (matrix header), and it implements `RelayTargets` (`:368`)
- **Also likely:** `checks/conformance/src/gateway.rs` — inferred: it implements `RelayTargets` (`:801`) and observes the `Relay` scenarios
- **Also likely (L6, L13, L15, K28, the pool):** `crates/llm-runpod/tests/runpod.rs` — inferred: tests named by row id for the hold, the timer and the startup sweep
- **Also likely:** `checks/conformance/src/runpod.rs` — inferred: its ensure, restore and reap steps (`:367`, `:424`, `:482`) are where hold and sweep scenarios would be observed
- **Also likely:** `docs/hosting.md` — inferred: the "startup deadline" row (`:301`) and the must-be-stepped paragraph (`:313-314`) describe exactly what this story changes
- **Also likely (the process):** `crates/llm-gateway-cli/src/serve.rs` — inferred: `start` marks the gateway ready (`:168`), so the sweep before serving and the reaper timer go there or beside it
- **Also likely (spec-first):** `spec/domains/gateway.yaml` — inferred: the `Refusal` enum (`:47-49`)
- **Also likely:** `spec/domains/runpod.yaml` — inferred: `PoolRefusal` (`:25`) plus the hold and timer intent
- **Also likely:** `contracts/ess-inputs.yaml`, `contracts/baseline.json`, `contracts/suite.json` — inferred: new scenarios are declared, raise the floor and regenerate the suite
- **Also likely:** `contracts/schema/schema/types/llm-gateway.gateway.Refusal.schema.json` — inferred: a new variant
- **Documents:** not a documents-only story; its document files are listed above with their own marks — cited (the acceptance requires a test per row)
- **Confidence:** medium — inferred: the three cited files are exact, but where the hold lives, whether `RelayTargets` changes shape, and where the timer runs are open design choices, and the pool adapter in the binary does not exist until story:gateway-deployment lands
- **Would collide with:** any unit touching `crates/llm-runpod/src/pool.rs`, the `RelayTargets` seam or the `RefusalCode` set in `crates/llm-gateway`, `crates/llm-gateway-cli/src/serve.rs`, or the shared matrix, spec and contracts files — inferred
- **Safety fact:** holding a request by calling `RunpodPool::ensure` again cannot start a second pod. Each call takes the pool mutex (`pool.rs:424`), and "a caller that is told `Starting` asks again; it never causes a second pod" (`pool.rs:412-414`). The hold must wait outside that mutex — cited, step 2, unproven

Not established: where the hold lives (the pool or the binary's adapter that story:gateway-deployment creates); whether W6 changes `RelayTargets::acquire`; how `start_wait_seconds` (`crates/llm-gateway-cli/src/config.rs:95-96`) becomes the hold budget; whether L13 and L15 are specified on the process or the pool. A held request occupies one of 64 concurrent slots (`server.rs:508`) and a stop waits for it (`docs/gateway.md:234`), so a hold must watch the drain flag.
