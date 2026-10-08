---
format: aep.planning-md/3
id: story:runpod-provider-description
kind: story
status: implemented
title: The Runpod adapter takes a pod's address from llm's Runpod description
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: inferred
  path: contracts/runpod/scenarios
- confidence: inferred
  path: crates/llm-runpod/Cargo.toml
- confidence: inferred
  path: crates/llm-runpod/src/emulated.rs
- confidence: cited
  path: crates/llm-runpod/src/lib.rs
- confidence: cited
  path: crates/llm-runpod/src/provider.rs
- confidence: inferred
  path: crates/llm-runpod/tests/adversary.rs
- confidence: cited
  path: crates/llm-runpod/tests/adversary2.rs
- confidence: cited
  path: crates/llm-runpod/tests/adversary_w02.rs
- confidence: cited
  path: crates/llm-runpod/tests/round2.rs
- confidence: inferred
  path: crates/llm-runpod/tests/runpod.rs
- confidence: inferred
  path: docs/hosting.md
- confidence: cited
  path: docs/llmgw-capability-matrix.md
- confidence: inferred
  path: spec/domains/runpod.yaml
revision: 18
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T13:35:57Z", actor: "human:timo", revision: 10}
- {from: "proposed", to: "active", at: "2026-10-08T13:35:57Z", actor: "human:timo", revision: 11}
- {from: "active", to: "implemented", at: "2026-10-08T14:10:01Z", actor: "human:timo", revision: 18, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

The Runpod adapter takes a running pod's inference address from llm's Runpod provider
description instead of a format string of its own. `crates/llm-runpod` depends on
`b10x-llm-providers` from beyond10x/llm by tag `0.5.0`
(https://github.com/beyond10x/llm/releases/tag/0.5.0), and a running pod's reported endpoint is
`llm_providers::descriptions::runpod().inference_base_url(<pod id>)`:
`https://<pod id>-8000.proxy.runpod.net/v1/` (llm `crates/llm-providers/descriptions/runpod.toml`).

## Why

Design choice D1 = A (`decision-blocker:runpod-control-plane`) writes Runpod's URLs down once, in
llm, for every consumer. The design names this adapter's hard-coded proxy URL
(`crates/llm-runpod/src/provider.rs:148`) as work removed here under A
(`docs/design/runpod-clients.md` § 4, row "Work removed here"). llm 0.5.0 ships the description.

## ESS first

`spec/domains/runpod.yaml` declares where the endpoint comes from and its form, including the
`/v1/` suffix, which llm's catalog `base_url` also requires (`docs/design/runpod-clients.md` § 3,
"`base_url` must carry `/v1`"). The scenarios that observe an endpoint are updated to the new form
and the suite is regenerated.

## Acceptance

1. A running pod reports `https://<pod id>-8000.proxy.runpod.net/v1/`, built by llm's
   `inference_base_url`; `crates/llm-runpod/src/provider.rs` holds no proxy host string. A test
   pins the value against `descriptions::runpod()`.
2. A pod whose id the description refuses (outside 1-48 bytes of `[a-z0-9]`) reports no endpoint,
   as a pod that is not running does, and a test pins it. The adapter never builds an address from
   such an id.
3. `EmulatedRunpod` issues ids the description accepts, so every scenario that observes an
   endpoint still observes one.
4. `crates/llm-runpod/Cargo.toml` takes the crate by tag, never by path (`AGENTS.md`,
   "Dependencies on llm"). `crates/llm-gateway/tests/dependency_boundary.rs` still passes: the
   gateway crate does not link it.
5. `docs/hosting.md` states the endpoint's form and its source, and the conformance check answers
   every scenario with none skipped.

## Depends on

Nothing open. Does not touch `spec/domains/deployment.yaml` or `crates/llm-gateway-cli`, which
`story:provider-key-files` changes in the same wave.

## Scope (inferred)

`crates/llm-runpod/Cargo.toml`, `crates/llm-runpod/src/provider.rs`,
`crates/llm-runpod/src/emulated.rs`, `crates/llm-runpod/tests/runpod.rs`,
`crates/llm-runpod/tests/adversary.rs`, `spec/domains/runpod.yaml`,
`contracts/runpod/scenarios/`, `docs/hosting.md`.
