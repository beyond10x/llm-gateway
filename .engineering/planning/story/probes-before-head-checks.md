---
format: aep.planning-md/3
id: story:probes-before-head-checks
kind: story
status: active
title: A probe or public route answers even when its head repeats a header
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: crates/llm-gateway/tests/gateway.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T03:10:17Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-30T03:10:17Z", actor: "human:timo", revision: 4}
---
## Outcome

A liveness or readiness probe, and a public route (`GET /`, `GET /v1/models`), is answered even when
its head repeats a header: `/health` with two `X-Probe` lines answers 200 `{"status":"live"}`, and
`/v1/models` with two `Authorization` lines answers 200 with the list.

## Why

`docs/gateway.md:103` says `/health` always answers 200 while the process serves, `:119-121` that the
probes are matched before anything else, and `:146-148` that the public routes answer without a
credential. The head's duplicate-header refusal (`crates/llm-gateway/src/server.rs:733-742`) runs
before the probe and public-route match (`server.rs:779-801`), so a repeated header is refused 400
`request-malformed`, or 401 `credential-malformed` for `Authorization`. A failing liveness probe
restarts the process (`docs/gateway.md:122-124`).

Found by the adversarial review of 2026-10-30 (claim S1), proven by the two tests below failing on
main 70841f2a.

## Acceptance

`review_s1_a_repeated_header_on_a_liveness_probe_is_still_answered_live` and
`review_s1_a_repeated_authorization_on_a_public_route_is_still_answered` in
`crates/llm-gateway/tests/gateway.rs` pass. Every other test in that file still passes.

## Scope (inferred)

`crates/llm-gateway/src/server.rs`, `crates/llm-gateway/tests/gateway.rs`, `docs/gateway.md` if the
order of decisions it lists changes, `CHANGELOG.md`.
