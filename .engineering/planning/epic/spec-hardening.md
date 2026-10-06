---
format: aep.planning-md/3
id: epic:spec-hardening
kind: epic
status: draft
title: The specification declares what the gateway and hosting documents promise
relations:
- serves: vision:portable-model-inference
revision: 2
---
## Outcome

The specification declares what `docs/gateway.md`, `docs/hosting.md` and the README's deployment
section promise. Every declared rule is pinned by a conformance scenario, by a crate test that
names the rule, or by an `ESS-LIMIT:` note saying why ESS cannot hold it. A change that narrows
the specification fails the gate until someone acknowledges it.

## Why

Operator request, 2026-10-06: harden the specification with `ess:hardening`. The record of that
run is `docs/verification/spec-hardening.md`. The two design reviews are enumerated row by row in
`docs/verification/spec-hardening-gateway-review.md` and
`docs/verification/spec-hardening-hosting-review.md`. The findings:

- **Design reviews.** 82 mismatches besides the planted ones: 37 for the gateway (2 contradicts,
  25 missing, 9 spec-only, 1 unclear) and 45 for hosting (2 contradicts, 1 stale pointer, 35
  missing, 3 unclear, 4 spec-only). The hardening change already fixed the four contradictions,
  which were stale documents: the README's trimming and file-mode rules, and two
  `docs/hosting.md` pointers. It also fixed the stale pointer.
- **Mutation audit.** `ess verify conform mutate` found no rule to break. Each command has one
  `observed` outcome, and the IR declares 0 guards, 0 invariants and 0 transitions.
- **Hand mutant.** Deleting `Requested -> Disowned` from `TRANSITIONS` was caught only by the
  scenario that reads the published table. No behaviour scenario drives that edge.
- **Spec diff.** Spec diffs are not gated. Adding a domain reports `unknown`
  (https://github.com/beyond10x/ess/issues/469).

## Not in scope

| Finding or technique | Why not |
| --- | --- |
| Guards and invariants for the gateway, deployment and Runpod domains, and their single `observed` outcome | Their behaviour is held by authored scenarios over a verification adapter, and the stories above declare its rules. Remodelling them as ESS lifecycles is not planned. The hosting state machine is, in `story:hosting-lifecycle-declared`, because it bounds spend. |
| Techniques 2 to 5 (random sequences, caller replay, determinism, metamorphic relations) | ESS ships its explorer only in the generated Go and TypeScript packages, and the implementation here is Rust. Once `story:hosting-lifecycle-declared` gives the specification a lifecycle, technique 2 can be reconsidered. |

## Done when

`story:gateway-spec-declarations`, `story:hosting-spec-declarations` and `story:spec-diff-gate`
are `implemented`. A second design review with the same brief returns no `missing` or
`contradicts` finding that is neither declared nor carried as an `ESS-LIMIT:` note.
`story:hosting-lifecycle-declared` is either `implemented`, with `ess verify conform mutate`
scoring at least one mutant, or `archived` with the reason written down.
