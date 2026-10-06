---
format: aep.planning-md/3
id: story:usage-records
kind: story
status: draft
title: Every model call yields a usage record with the token counts the target reported
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:gateway-observability
- depends_on: story:target-fallback
scope:
- confidence: inferred
  path: checks/conformance/Cargo.toml
- confidence: inferred
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/usage.rs
- confidence: inferred
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-gateway/tests/dependency_boundary.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/telemetry.yaml
revision: 19
---
## Outcome

For every model call it relays, the gateway's usage record (`llm-gateway.telemetry.UsageRecord`)
carries the token counters the target reported, along with `reported_model`. A counter the target
did not report stays absent. The record's other fields need no parsing, and
`story:gateway-observability` owns them.

## Why

`vision:portable-model-inference` moves objective O6, attributable usage and cost. Claude Code and
Codex reach the gateway directly and are not llm clients, so llm's caller-side cost ledger never
sees their calls. Envoy, Kong, LiteLLM and Cloudflare all record token usage per request
(`epic:gateway-features`).

## Blocked

`decision-blocker:usage-from-responses` was cleared on 2026-10-06 with option A: the gateway parses a copy
of every answer, using llm's protocol crates taken by tag, behind a port the binary implements
(`spec/domains/telemetry.yaml`, `DECIDED 2026-10-06`).

## ESS first

`llm-gateway.telemetry.UsageRecord` in `spec/domains/telemetry.yaml` is marked `PLANNED`. This
story adds the token fields to the observation that `story:gateway-observability` creates for the
record, and adds the scenarios.

## Acceptance

Each case is a `Relay` conformance scenario over loopback fixture targets, and reads the record
from the observation. None makes a paid call.

1. On each of the three wires, a fixture that reports usage yields a record with exactly those
   counters and the model name the fixture reported. This holds both for a streamed answer and
   for one that is not streamed.
2. A fixture that reports no usage and no model yields a record with every counter absent and
   `reported_model` absent. A reported zero is recorded as zero.
3. A call that ends `upstream-failed` yields a record with every counter absent.
4. A call whose position 0 answers 503 and whose position 1 reports usage yields one record,
   carrying position 1's counters only.
5. The client receives the target's answer byte for byte, so the existing `Relay` scenarios in
   `contracts/gateway/scenarios/` still pass.
6. This story adds one token series per counter, per model and wire. Their names follow the scheme
   `story:gateway-observability` settles in `spec/domains/telemetry.yaml`. After case 1, each
   series has grown by exactly the reported count.

## Depends on

- `story:gateway-observability`, which creates the record, its observation and the naming scheme
  for series.
- `story:target-fallback`. Both stories change `relay::serve` in `crates/llm-gateway/src/relay.rs`,
  and one record per call has to account for the attempt loop that story adds (case 4).

## Scope (inferred)

Superseded by `## Scope`, which `story-scoper` derived on 2026-10-06. The typed entries are in the
frontmatter `scope`.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-gateway/src/relay.rs`: the answer path in `relay::serve` (`:537`) — cited
- **Files:** `crates/llm-gateway/src/relay.rs:537` (`serve`), where one record per call is closed, `upstream-failed` included — cited
- **Files:** `crates/llm-gateway/src/relay.rs:626-675` (`stream_answer`), where each decoded piece is copied to the port after its `write_by` — inferred
- **Files:** `spec/domains/telemetry.yaml:59-78` (`UsageRecord`, token fields `:73-78`) plus one token `MetricSeries` per counter (`:52`) — cited
- **Files:** `contracts/ess-inputs.yaml`, which lists every new scenario (AGENTS.md "Generated files") — cited
- **Files:** `contracts/suite.json`, regenerated from the spec and the scenarios — cited
- **Files:** `crates/llm-gateway-cli/Cargo.toml`, which takes llm's protocol crates by tag (decision A, AGENTS.md "Dependencies on llm") — cited
- **Files:** `Cargo.lock`, which a by-tag git dependency changes under the `--locked` gate — cited
- **Files:** `crates/llm-gateway-cli/src/usage.rs` (new), the binary's implementation of the parsing port — inferred
- **Files:** `crates/llm-gateway-cli/src/lib.rs`, its `mod` line — inferred
- **Files:** `crates/llm-gateway-cli/src/serve.rs:166`, where the binary would compose the relay with the port (today it binds without a `Relay`) — inferred
- **Files:** `crates/llm-gateway/src/lib.rs:43`, the re-export of the port trait — inferred
- **Files:** `checks/conformance/src/gateway.rs`: `Relay::new` at `:902`, fixture pods `answer_one` at `:662` that must report usage, the `LastRelay` view at `:863` — inferred
- **Files:** `checks/conformance/Cargo.toml`, because the runner needs a port implementation, either the CLI lib or llm by tag — inferred
- **Symbols:** `relay::serve`, `llm-gateway.telemetry.UsageRecord`, `reported_model` — cited
- **Also likely:** `contracts/baseline.json`, floors 179/179 raised for the new scenarios — inferred
- **Also likely:** `docs/gateway.md`, the contract text on what the relay reads — inferred
- **Also likely:** `crates/llm-gateway/tests/dependency_boundary.rs:22-33`, adding `b10x-llm-chat`/`-messages`/`-responses`/`-core` to `FORBIDDEN` — inferred
- **Also likely:** `spec/domains/gateway.yaml:160` (`RelayObservation`), if the record's observation lives there — inferred
- **Confidence:** medium — the story cites `relay.rs` and `telemetry.yaml`, but where the port goes and how the CLI and conformance are wired is inferred, and the binary composes no relay until `story:gateway-deployment` lands
- **Would collide with:** `relay.rs` (`serve`, `RelayTargets`); `spec/domains/telemetry.yaml`; `crates/llm-gateway-cli/src/serve.rs`; `checks/conformance/src/gateway.rs`; `contracts/ess-inputs.yaml`, `contracts/suite.json`, `contracts/baseline.json`; `Cargo.lock` and `checks/conformance/Cargo.toml` (any dependency or ESS-pin move) — cited for the first two through the story's own `depends_on` edges, inferred for the rest
- **Safety fact:** the client's bytes cannot change: the port sees only a copy taken after each `write_by` in `stream_answer` (`relay.rs:650-675`), and a parse error or panic there must not reach that loop's `return` paths. This is design, not code yet (level 1, unproven). Also level 2, unproven: llm 0.2.0's protocol crates pull `b10x-llm-http`, `tokio` and `reqwest`, all in `FORBIDDEN` (`dependency_boundary.rs:22-33`), so linking them into the gateway crate fails that test.

Not established, and decisive for when this story can run: at llm 0.2.0, chat's `usage_of` is private (`incoming.rs:426`), messages' `Snapshot` is `pub(crate)` (`usage.rs:7`), and `b10x-llm-responses` exports only `decode_stream(binding, &[Value])`, which needs `stream: true` (`request.rs:168`) and the whole stream as one slice. Acceptance case 1 on the responses wire may need a change in beyond10x/llm first. All three protocol crates also pull `b10x-llm-credentials`, `b10x-llm-http`, `reqwest` and `tokio` into the binary.
