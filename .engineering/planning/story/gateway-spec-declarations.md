---
format: aep.planning-md/3
id: story:gateway-spec-declarations
kind: story
status: implemented
title: gateway.yaml and deployment.yaml declare every rule docs/gateway.md and README promise
relations:
- decomposes: epic:spec-hardening
- serves: vision:portable-model-inference
- depends_on: story:ess-0-53
scope:
- confidence: cited
  path: README.md
- confidence: cited
  path: checks/conformance/src/gateway.rs
- confidence: cited
  path: contracts/gateway/scenarios
- confidence: cited
  path: crates/llm-gateway-cli/tests/binary.rs
- confidence: cited
  path: crates/llm-gateway/tests/spec_declarations.rs
- confidence: cited
  path: docs/gateway.md
- confidence: cited
  path: docs/verification/2026-10-08-gateway-spec-declarations.md
- confidence: cited
  path: docs/verification/spec-hardening-gateway-review.md
- confidence: cited
  path: spec/domains/deployment.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: spec/system.yaml
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T20:01:23Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "proposed", to: "active", at: "2026-10-08T20:01:24Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"review_outcome":3}}}
- {from: "active", to: "implemented", at: "2026-10-08T21:43:29Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":1}}}
---
## Outcome

`spec/domains/gateway.yaml` and `spec/domains/deployment.yaml` declare every behaviour that
`docs/gateway.md` and the README's deployment section promise. Every declaration they make is
justified by one of those documents.

## Why

The 2026-10-06 design review (`docs/verification/spec-hardening.md`) found 25 rules missing, 9
spec-only declarations and 1 unclear statement for the gateway. Examples: the status and message
of 11 of the 19 refusal codes, the probe paths and bodies, the inspection decision order the
relay defers to, `connection: close`, the one-deadline body read, the stop bound of twice
`read_timeout`, and the bound defaults.

## Acceptance

The findings are the rows of `docs/verification/spec-hardening-gateway-review.md` whose status is
`open`: rows 4-28 are `missing`, rows 29-37 are `spec-only`, and row 38 is `unclear`.

1. For each of rows 4-28, the specification declares the rule. It cites an existing scenario in
   `contracts/gateway/scenarios/` or a row-named test that pins the rule, or this story adds one.
   A rule ESS cannot hold is carried as an `ESS-LIMIT:` note naming the test that does.
2. Each of rows 29-37 is resolved one of two ways: the declaration is deleted, or a line added to
   `docs/gateway.md` or the README justifies it. The tests that `include_str!` the document still
   pass.
3. `llm-gateway.gateway.Refusal` lists every code in `docs/gateway.md`'s refusal table. Each
   variant's comment gives the code's status and message.
4. Row 38 asks whether the binary serves the three wire paths. The answer, read from the code at
   the story's base commit, is written in two places: a `DECIDED` line in the header of
   `spec/domains/deployment.yaml`, and a sentence in README's "Run the gateway" section.
5. The review file's status column names, for each row, the declaration, test or note that
   closes it.
6. The design-review brief from `ess:hardening` is re-run against the two domains. It returns no
   `missing` or `contradicts` finding that is neither declared nor an `ESS-LIMIT:`. The run is
   recorded in a new `docs/verification/` record.
7. `ess specify validate` and the conformance check exit 0.

## Scope (confirmed by the implementor, wave 2026-10-08-w06)

Confirmed: `spec/domains/gateway.yaml`, `spec/domains/deployment.yaml`,
`checks/conformance/src/gateway.rs` (the new `answer_headers` fact), `docs/gateway.md`.

Corrected: the new record is `docs/verification/2026-10-08-gateway-spec-declarations.md` (the
dated-record convention), not `docs/verification/gateway-spec-declarations.md`.

Not listed but changed: `spec/system.yaml` (format `ess/14` to `ess/23`, for typed `Refusal`
attributes), `crates/llm-gateway/tests/spec_declarations.rs`,
`crates/llm-gateway-cli/tests/binary.rs`, four scenarios
`contracts/gateway/scenarios/inspection-*.yaml`,
`docs/verification/spec-hardening-gateway-review.md`, and `README.md` (row 38's answer and the
stop bound of row 22).
