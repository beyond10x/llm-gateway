---
format: aep.planning-md/3
id: story:gateway-observability
kind: story
status: active
title: llm-gateway exposes its counters and logs
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:cold-start-hold
scope:
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: inferred
  path: checks/conformance/src/target.rs
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/main.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/adversary.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: inferred
  path: crates/llm-gateway/src/lib.rs
- confidence: cited
  path: crates/llm-gateway/src/relay.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: inferred
  path: crates/llm-gateway/tests/gateway.rs
- confidence: cited
  path: crates/llm-runpod/src/pool.rs
- confidence: inferred
  path: crates/llm-runpod/tests/runpod.rs
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/domains/telemetry.yaml
revision: 27
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T15:31:44Z", actor: "human:timo", revision: 26, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-08T15:31:44Z", actor: "human:timo", revision: 27, decided_on: {"recorded":{"review_outcome":4}}}
---
## Outcome

`/metrics` exposes process-wide and per-model, per-wire counters covering llmgw's 13 metric names, and logging follows `RUST_LOG`.

## Rows

R4, O1, O2, O3, from `docs/llmgw-capability-matrix.md` (story:llmgw-capability-matrix). Each
row there cites the llmgw source this story must match or replace.

## Acceptance

- Rows R4, O1, O2 and O3 are each `covered` in the matrix, with the llm-gateway citation updated
  and a test that names the row id.
- `spec/domains/telemetry.yaml` no longer carries the `UNMAPPED` markers on series names or on
  where a record goes. Each is rewritten as `DECIDED`.
- The `Relay` conformance scenarios read one `UsageRecord` per request from the observation this
  story adds. They cover a relayed answer, a refusal and an `upstream-failed` call, and they check
  `disposition`, `refusal`, `status`, `response_bytes` and `duration_ms`. The token fields are
  absent.
- After those scenarios, each of the six series the record feeds has grown by exactly the
  records counted.

## Why

llm `story:llmgw-retirement`: no deployment moves from llmgw until every row is covered or not
needed.

## Per-call record

The gateway makes one record per model call, `llm-gateway.telemetry.UsageRecord` in
`spec/domains/telemetry.yaml`. This story fills the fields that need no parsing: `model`, `wire`,
`disposition`, `refusal`, `status`, `response_bytes` and `duration_ms`. It also adds the
conformance observation the record is read from, and settles the specification's markers on the
series names and on where a record goes. The token fields and their series belong to
`story:usage-records`.

The record feeds six of llmgw's 13 series: `inference_requests_total`, `upstream_failures_total`,
`route_requests_total`, `route_refusals_total`, `route_upstream_status_failures_total` and
`route_response_bytes_total`. The other seven are counted where their events happen.
`instruction_views_total` counts `GET /` (row R1, `story:public-model-listing`).
`endpoint_invalidations_total` counts target-source invalidations (row W7). `pod_starts_total`,
`pod_start_failures_total` and `pod_reaps_total` are counted in the pool.
`cold_start_wait_seconds_total` and `route_cold_start_holds_total` count the hold that
`story:cold-start-hold` adds, which this story depends on.

## Neighbours

These stories share files with this one:

- `story:public-model-listing` adds `GET /` and `GET /v1/models` (rows R1, R5) to the route table
  in `crates/llm-gateway/src/server.rs`, where this story adds `GET /metrics` (R4).
- `story:cold-start-hold` hooks the relay path in `crates/llm-gateway/src/relay.rs` that
  `route_cold_start_holds_total` counts.

Do not schedule any of the three in one wave without an order between them
(`review-result:gateway-features-parallel-safety-round-1`).

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-gateway/src/server.rs` and `crates/llm-gateway/src/relay.rs` — cited, the story's Neighbours section
- **Files:**
  - `crates/llm-gateway/src/server.rs:491-520` (`decide`, where `GET /metrics` joins `/health` and `/ready`), `:268-311` (`serve`, where a relay refusal becomes `Outcome::Refused` at `:294`) — cited
  - `crates/llm-gateway/src/relay.rs:537-621` (`relay::serve`: model, wire, refusal, 502 path) and `:626-676` (`stream_answer`: status and response bytes) — cited
  - `crates/llm-runpod/src/pool.rs:134-145` (`CleanupReport`; `pod_starts_total`, `pod_start_failures_total` and `pod_reaps_total` are counted in the pool) — cited, matrix row O1 and the story's Per-call record section
  - `spec/domains/telemetry.yaml:13-16` (series-name marker) and `:31-34` (record-destination marker) — cited
  - `checks/conformance/src/gateway.rs:470` (`mod relay`), `:859` (`exercise`), `:1077-1144` (the Relay facts) — cited, Relay is run here and the acceptance reads the record from Relay scenarios
  - `contracts/ess-inputs.yaml` — cited, `checks/conformance/src/gate.rs:72-101` refuses an unlisted scenario
  - `contracts/suite.json` — cited, `checks/conformance/src/gate.rs:128-130` drift check is rewritten by any added scenario
- **Symbols:** `relay::serve`, `stream_answer`, `decide`, `RelayTargets::invalidate` (`relay.rs:578`), `CleanupReport`, `llm-gateway.telemetry.UsageRecord`, `llm-gateway.gateway.RelayObservation` — cited
- **Also likely:**
  - `spec/domains/gateway.yaml:160-188` (`RelayObservation`) and `:202-230` (`Relay` program) — inferred, if the record and the `/metrics` scrape become Relay facts rather than a new telemetry entity
  - `contracts/schema/schema/entities/llm-gateway.gateway.RelayObservation.schema.json` — inferred, regenerated if `RelayObservation` gains fields
  - `checks/conformance/src/target.rs:96`, `:124-128` — inferred, only if the observation becomes a second view (one observed view per command today)
  - `crates/llm-gateway/src/lib.rs:37-44` — inferred, re-exports of a record port and a metrics type
  - `crates/llm-gateway-cli/src/serve.rs:160-170` — inferred, composes the record sink; `:166` binds with no relay today
  - `crates/llm-gateway-cli/src/main.rs:24-26` — inferred, `RUST_LOG` setup for O3 replaces or wraps `report`
  - `crates/llm-gateway-cli/Cargo.toml` — inferred, `tracing` dependencies for O3
  - `Cargo.lock` — inferred, same reason
  - `crates/llm-gateway-cli/tests/binary.rs:24-25`, `:609`, `:629` — inferred, these assert exact stderr lines that O3 may change
  - `crates/llm-gateway-cli/tests/adversary.rs:19`, `:68` — inferred, same stderr lines
  - `crates/llm-gateway/tests/gateway.rs` — inferred, the row-named tests for R4 and O2
  - `crates/llm-runpod/tests/runpod.rs` — inferred, the row-named test for the O1 pod counters
- **Documents:**
  - `docs/llmgw-capability-matrix.md:50` (R4), `:161-163` (O1-O3), `:38`, `:41` (summary), `:181`, `:196-198` (gaps) — cited
  - `docs/gateway.md:90-100` — inferred, the HTTP surface table gains `/metrics`
  - `spec/domains/deployment.yaml:14-18` — inferred, the stderr-line contract, if O3 changes those lines
  - `README.md:111-115` — inferred, same condition
- **Confidence:** medium — the story names `server.rs`, `relay.rs`, `telemetry.yaml` and the matrix. The CLI, conformance-view and document lines depend on decisions the story has not made yet (where a record goes, and how the observation is shaped).
- **Would collide with:** any unit editing the route dispatch in `server.rs` (`decide`/`inspect`), the attempt or answer path in `relay::serve`, the Relay harness in `checks/conformance/src/gateway.rs`, the pool's start/reap path in `pool.rs`, the composition in `crates/llm-gateway-cli/src/serve.rs`, or any unit that adds scenarios (`contracts/suite.json`, `contracts/ess-inputs.yaml`) or flips a matrix row — cited for the code files, inferred for the CLI
- **Safety fact:** `llm-gateway` declares no dependencies and calls no output functions. `crates/llm-gateway/tests/dependency_boundary.rs:152` asserts it declares nothing, `:203` asserts it links nothing but itself, and `crates/llm-gateway/tests/gateway.rs:1213-1232` forbids `eprintln!`, `io::stderr` and `tracing::` in its sources. So the Prometheus text must be rendered by hand inside the crate, and the record and logs must leave it through a port the binary implements (as `RelayTargets` does). A `tracing` or Prometheus dependency on `llm-gateway` fails both tests — level 2, unproven

Not established: whether `GET /metrics` needs the owner credential (unauthenticated breaks the closed two-path probe rule at `docs/gateway.md:100`); where the two cold-start counters and `endpoint_invalidations_total` are counted; whether O3 makes `llm-runpod` log too. Until story:gateway-deployment wires the relay and the pool, `/metrics` shows zeros for pod and relay series.
