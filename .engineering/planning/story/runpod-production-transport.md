---
format: aep.planning-md/3
id: story:runpod-production-transport
kind: story
status: draft
title: Runpod has a production REST/GraphQL transport
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
- depends_on: story:hosting-spec-declarations
- depends_on: story:spec-diff-gate
scope:
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: checks/conformance/src/runpod.rs
- confidence: inferred
  path: crates/llm-runpod/Cargo.toml
- confidence: cited
  path: crates/llm-runpod/src/lib.rs
- confidence: inferred
  path: crates/llm-runpod/src/provider.rs
- confidence: inferred
  path: crates/llm-runpod/src/rest.rs
- confidence: cited
  path: crates/llm-runpod/src/transport.rs
- confidence: inferred
  path: crates/llm-runpod/tests/transport.rs
- confidence: cited
  path: docs/hosting.md
- confidence: inferred
  path: docs/verification/runpod-transport.md
- confidence: cited
  path: spec/domains/runpod.yaml
revision: 20
---
## Acceptance

Rewritten 2026-10-08 for design choice D1 = A (`decision-blocker:runpod-control-plane`,
`docs/design/runpod-clients.md` § 4). Rows B2, B3, B4 and B5 of
`docs/llmgw-capability-matrix.md` are `covered` by a production `RunpodTransport` whose control
plane calls go through beyond10x/connectors' Runpod bundle; row B6 (the readiness probe) is
covered by a call to the pod's address from llm's description. Tests run only against fixtures,
and no test makes a paid call.

1. Create, list, get and terminate invoke the connectors operations llm 0.5.0's Runpod
   description names (`CreatePod`, `ListPods`, `GetPod`, `DeletePod`;
   `crates/llm-providers/descriptions/runpod.toml` at llm tag `0.5.0`), through the interface
   the connectors release that carries the bundle publishes. This crate holds no HTTP client and
   no credential for the control plane. A test against a fixture of that interface records each
   invocation.
2. A create answers `CreateAnswer::Refused` only when connectors classifies the write `refused`;
   connectors' `unknown`, a timeout or an unparseable answer is `Lost`, and a test pins each case.
   With the classification inverted, the provider's GPU fallback (`crates/llm-runpod/src/provider.rs`,
   `fn create` and its GPU loop) submits a second create, and that test fails.
3. `PodListing::complete` (`transport.rs:41`) is `true` only when the listing is whole. Tests pin
   `false` for a truncated listing, a throttled answer and an interrupted one.
4. The readiness probe sends the model's vLLM key, read by `story:provider-key-files`, as a
   bearer to `<endpoint>models`, the endpoint `story:runpod-provider-description` reports. The
   transport reads no file, and its `Debug` prints no key, which a test checks.
5. Crash-loop detection reads `lastStartedAt` moving forward, in place of llmgw's GraphQL uptime
   query (`docs/design/runpod-clients.md` § 4, marked there as inferred and unverified); a test
   pins the rule and `docs/hosting.md` states it.
6. `spec/domains/runpod.yaml` declares the invocations and replaces its `DEFERRED:` note on the
   transport before any code. `docs/verification/runpod-transport.md` records the fixtures and
   the planted `Refused`/`Lost` inversion.
7. The paid-call row of `AGENTS.md` "Invariants" reads: "No test makes a paid call or provisions
   an external resource. Tests run against `EmulatedRunpod` or a fixture, never a live API."

## Moved

Moved from `beyond10x/llm` (`story:runpod-production-transport`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/runpod-production-transport/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.

## Scope

Derived 2026-10-06 by `story-scoper`. Every line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/llm-runpod/src/transport.rs` — cited, the `DEFERRED:` note at `spec/domains/runpod.yaml:11-14` points at `transport.rs:5-7`
- **Rows:** B2, B3, B4, B5, B6 of `docs/llmgw-capability-matrix.md` — cited, `transport.rs:5-7` names exactly these calls and Gaps lines 186-190 say "A production Runpod transport …"
- **Rows not here:** K7 and B9 belong to story:provider-key-files and B8 to story:gateway-deployment (split 2026-10-06); this transport takes the Runpod key and the per-model vLLM key as inputs and reads no file — cited
- **Files:** `crates/llm-runpod/src/transport.rs:5-7,95-112` — cited, the trait to implement and the "none is written here" claim
- **Files:** `spec/domains/runpod.yaml:11-14` — cited, the `DEFERRED:` note this story owns; the REST/GraphQL request shapes are declared here first (spec-first)
- **Files:** `crates/llm-runpod/src/lib.rs:26-28,45-47` — cited, "opens no connection and reads no credential" (matrix K7) and the re-exports
- **Files:** `docs/hosting.md:289-291,326-330` — cited, "the only transport … is `EmulatedRunpod`" and the unchecked live control-plane assumptions; AGENTS.md makes this document a contract changed in the same commit
- **Files:** `docs/llmgw-capability-matrix.md:126-130,186-190` — cited, rows B2-B6 and their Gaps lines
- **Files:** `AGENTS.md:18,39` — cited, line 39 names this story and requires its tests to stay off paid calls
- **Files:** `README.md:25,38` — cited, both say `EmulatedRunpod` is the transport
- **Also likely:** `crates/llm-runpod/src/rest.rs` — inferred, a new module for the production transport; the name is a guess
- **Also likely:** `crates/llm-runpod/tests/transport.rs` — inferred, loopback-fixture tests; the name is a guess
- **Also likely:** `crates/llm-runpod/Cargo.toml` — inferred, adds an HTTP and TLS client; today it depends only on `llm-provision`
- **Also likely:** `Cargo.lock` — inferred, the lockfile holds no tokio, hyper, reqwest, ureq or rustls today
- **Also likely:** `Cargo.toml` — inferred, only if the client goes into `[workspace.dependencies]`
- **Also likely:** `crates/llm-runpod/src/provider.rs` — inferred, `probe_ready(pod_id)` carries no model or key, and the proxy URL is formatted at `:146-148`
- **Also likely:** `contracts/runpod/scenarios/` new scenario files, `contracts/ess-inputs.yaml`, `contracts/baseline.json`, `contracts/suite.json`, `contracts/schema/schema/entities/llm-gateway.runpod.ExecutionObservation.schema.json`, `checks/conformance/src/runpod.rs` — inferred, the regeneration and observation that follow a `runpod.yaml` change
- **Also likely:** `docs/verification/runpod-transport.md` — inferred, a new dated record (AGENTS.md: add a record, do not edit old ones)
- **Conditional, only if the trait signature changes:** `crates/llm-runpod/src/emulated.rs`, `checks/conformance/src/runpod.rs`, `crates/llm-runpod/tests/adversary.rs`, `crates/llm-runpod/tests/adversary2.rs` — inferred, these hold the four `impl RunpodTransport for` blocks
- **Symbols:** `RunpodTransport`, `CreateAnswer`, `PodListing::complete`, `Probe::Refused`, `RunpodProvider` — cited
- **Documents:** `docs/hosting.md`, `docs/llmgw-capability-matrix.md`, `AGENTS.md`, `README.md` — cited
- **Confidence:** medium — the deferral note and matrix pin the crate and the calls, but the story had no acceptance and no module name, and whether the trait changes is undecided
- **Would collide with:** `spec/domains/runpod.yaml` and `docs/hosting.md`, which story:orphan-termination-obligations also owns (`runpod.yaml:15-19`, `hosting.md:316-324`); `crates/llm-runpod/src/provider.rs` and `lib.rs`; the regenerated `contracts/suite.json`, `contracts/baseline.json` and `contracts/ess-inputs.yaml` (every story that changes the spec or adds a scenario); `docs/llmgw-capability-matrix.md` (every story that closes a row); `Cargo.lock`
- **Safety fact:** the transport may answer `CreateAnswer::Refused` only when Runpod definitely created nothing. Any timeout or ambiguous answer must be `Lost`, because `Refused` makes the provider try the next GPU (`provider.rs:246`) and Runpod takes no idempotency key (`provider.rs:315`), so a misclassified timeout can bill two pods. Proof level 2 (`file:line`), unproven

Not established: whether `RunpodTransport` must change (B6 needs a per-model vLLM key that `probe_ready(pod_id)` does not carry); whether blocking network calls under the pool's `std::sync::Mutex` (`pool.rs:312,406`) are acceptable; which HTTP client (none exists in the workspace; llm's `b10x-llm-http` by tag is the alternative); whether a loopback HTTP fixture satisfies `AGENTS.md:39` "stay on the emulator"; the live endpoint paths (`docs/hosting.md:326-328` says they were never checked).

## Depends on

`story:hosting-spec-declarations`. Both stories change `spec/domains/runpod.yaml` and
`docs/hosting.md`. This story declares the transport's request shapes on top of the Runpod rules
that story declares.

## Scope correction for D1 = A

Under D1 = A (2026-10-08) the Scope section below is corrected in three places: the inferred
`crates/llm-runpod/src/rest.rs` and "an HTTP and TLS client" for the control plane are dropped,
because connectors makes the Runpod API calls; the proxy URL is no longer formatted at
`provider.rs:146-148` (it comes from llm's description since `story:runpod-provider-description`);
and `provider.rs:315` no longer holds the "no idempotency key" note, so the double-billing fact
below is re-read from `docs/hosting.md` "GPU choice" when the story starts.
