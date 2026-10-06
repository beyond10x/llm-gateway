# Specification hardening, 2026-10-06

This record covers the `ess:hardening` techniques run against `spec/` at llm-gateway `a11ef83`,
with the planned `llm-gateway.telemetry` and `llm-gateway.upstream` domains in place. The
conformance suite was green before any of them ran: 179 of 179 scenarios passed on three
consecutive runs, and the projection drift check passed. Every technique saw its planted defect
fail before its green result was counted.

## Results

| # | Technique | Planted defect and what it produced | Green result after restoring | Findings |
| --- | --- | --- | --- | --- |
| 8 | Design review, `docs/gateway.md` and the README deployment section against `gateway.yaml` and `deployment.yaml` | `RequestTooLarge` deleted from `llm-gateway.gateway.Refusal` in a copy of the specification; reported as `missing`, citing `gateway.md:196` | not applicable: the review reads the copy | 37 besides the plant: 2 contradicts, 25 missing, 9 spec-only, 1 unclear |
| 8 | Design review, `docs/hosting.md` against `hosting.yaml` and `runpod.yaml` | `Disowned` deleted from `llm-gateway.hosting.Phase` in a copy; reported as `contradicts`, citing `hosting.md:77` and `:85` | not applicable | 45 besides the plant: 2 contradicts, 1 stale pointer, 35 missing, 3 unclear, 4 spec-only |
| 7 | Spec diff, `ess verify diff` (ESS 0.53.0; 0.52.0 has no compatibility gate) | `LeaseExpired` removed from `llm-gateway.hosting.StopReason` in a copy: exit 4, `breaking type/llm-gateway.hosting.StopReason/variant-removed/LeaseExpired` | the specification against itself: exit 0 | `origin/main` (00895dd) to this revision: 16 changes, 15 `compatible`, 1 `unknown` (`system/llm-gateway/unclassified-changed`). The unknown is the two domains added to `system.yaml`; reordering `domains:` alone reports nothing. Filed as https://github.com/beyond10x/ess/issues/469 |
| 1 | Mutation audit, `ess verify conform mutate --emit` (ESS 0.52.0) and this repository's runner | not reached | baseline suite: 4 of 4 passed | 4 mutants, all `emit-drop`, all stillborn (`ESS-MUTATE-002`). Every command has one `observed` outcome, so the specification gives `mutate` no rule to break |
| 1 | Mutation audit by hand, against the 179-scenario authored suite | `SERVED_EFFORT` changed to `"high"` in `crates/llm-gateway/src/body.rs`: 178 of 179, `w4-on-the-messages-wire-effort-high-reaches-the-target-as-xhigh` failed | 179 of 179, three runs, exit 0 | killed |
| 1 | Mutation audit by hand | `(Phase::Requested, Phase::Disowned)` deleted from `TRANSITIONS` in `crates/llm-provision/src/machine.rs`: 178 of 179, `the-ownership-state-machine-is-exactly-this-table` failed | 179 of 179, three runs, exit 0 | killed, but only by the scenario that reads the published table. No behaviour scenario drives `Requested` to `Disowned` |
| 6 | Guard analysis over the compiled IR | not reached | not applicable | the IR declares 0 guards, 0 invariants and 0 transitions, so there is nothing to analyse |

The two design reviews are enumerated row by row, with a status per row, in
[spec-hardening-gateway-review.md](spec-hardening-gateway-review.md) and
[spec-hardening-hosting-review.md](spec-hardening-hosting-review.md).

## Findings that changed this revision

- README said the owner secret has "a trailing newline or CRLF" trimmed and must be "readable by the owner only". The code trims all trailing ASCII whitespace (`crates/llm-gateway-cli/src/serve.rs:50`) and refuses any group or world permission (`crates/llm-gateway-cli/src/trusted.rs:42`, `0o077`), as `deployment.yaml` says. README now agrees with the code.
- `docs/hosting.md` named the domain `llm.hosting` and a `spec/domains/catalog.yaml` this repository does not have; `hosting.yaml` pointed at a `docs/design.md` that does not exist; `gateway.yaml` pointed at "the credential scenarios below", which live in `contracts/gateway/scenarios/`. All four pointers are corrected.

## Findings left to the plan

The other design-review findings are rules the documents state that the specification declares
only by observing them, or not at all. Among them: the status and message of 11 of the 19 gateway
refusal codes, the probe surface, and the order of inspection decisions. On the hosting side: the
`HostingError` codes, the evidence vocabulary, the lease-per-deployment rule and the Runpod
safety rules. `epic:spec-hardening` owns them. Three hosting statements are unclear and need
their author:

- whether the crash-restart limit is fixed at two or configurable (`hosting.md:300` against
  `:309`; `crates/llm-runpod/src/config.rs` has a `crash_restart_limit` field);
- which `ProviderState` an exited pod maps to (`hosting.md:305`);
- whether `pod-exited` and `stopped-serving` are refusals a caller receives (`hosting.md:301`,
  `:305`).

## Not run

Techniques 2 (random sequences), 3 (caller replay), 4 (determinism) and 5 (metamorphic relations)
were not run. ESS ships the explorer only in its generated Go and TypeScript packages, the
implementation here is Rust, and the specification has no lifecycle or guard for a reference
model to step through.
