# Hosting lifecycle mutation audit, 2026-10-30

The mutation audit for `story:hosting-lifecycle-declared`, on `impl/hosting-lifecycle-declared`
(base `5d5bf61`), after `llm-gateway.hosting.OwnedResource` declared the ownership state machine
(`crates/llm-provision/src/machine.rs` `Phase`, `TRANSITIONS`) as its ESS lifecycle. `ess` is
0.77.0; the system is on `format: ess/23`. [hosting.md](../hosting.md), "The table as a declared
lifecycle", describes the lifecycle and the commands that move it.

The run below is on the specification as corrected after the adversary pass:
`Provision` and `Retry` declare `stale-epoch` in `Disowned` and name the held states of every
branch, the view `OwnedResources` publishes every field of the record, the identifiers are typed,
and `ConfirmStopped` declares `invalid-identifier`. The run before that correction had the same
verdicts over an 83-scenario baseline.

## What was run

From the repository root, with the conformance binary built from the same tree:

```console
ess verify conform mutate --path spec --class from-drop --class transition-to --emit <dir>
b10x-llm-gateway-conformance run <dir>/<mutant>/suite.json <zero floors> <dir>/<mutant> mutation-audit
ess verify conform mutate --collect <dir> --report-out <report>
```

The second line ran once for the baseline and once for each emitted mutant, with floors of zero
(`{"answered_floor": 0, "total_floor": 0, "skipped_ceiling": 0}`), because a mutant suite is the
synthesized suite alone and does not carry the authored scenarios. The runner exits 1 on every
suite, the baseline included: its gate refuses a run whose conformance status is not `passed`, and
a synthesized-only suite is `inconclusive`. The collector reads each `report.json`, not the exit
code. The baseline report is `{"total":92,"passed":92,"failed":0,"error":0,"unsupported":0,"skipped":0}`.

## Result

```text
mutation audit of llm-gateway v1 against llm-gateway-serving-libraries mutation-audit: 27 mutant(s), 23 killed, 0 survived, 0 inconclusive, 4 stillborn, 0 unwitnessed, 0 equivalent (baseline: 92 scenario(s), 0 refusal(s))
```

`--collect` exited 0. Every one of the twelve transitions has at least one killed mutant.

| Transition | Mutant | Verdict | Killed by (scenarios that failed) |
| --- | --- | --- | --- |
| `submit` | `to: Requested` becomes `to: Active` | killed | `state/Active/accepts/ObserveUnlisted`, `state/Active/refuses/Provision`, five more (7) |
| `lose` | `to: Uncertain` becomes `to: Active` | killed | `state/Active/refuses/Provision`, `transition/lose/by/Provision/lost`, four more (6) |
| `misanswer` | `to: Uncertain` becomes `to: Active` | killed | `state/Active/refuses/Provision`, `transition/misanswer/by/Provision/misanswered`, four more (6) |
| `withdraw` | `to: Cancelled` becomes `to: Active` | killed | `ConfirmStopped/outcome/confirmed`, `ObserveListed/outcome/adopted`, 13 more (15) |
| `cancel` | `to: Cancelled` becomes `to: Active` | killed | `Cancel/outcome/cancelled`, `ConfirmStopped/outcome/confirmed`, 36 more (38) |
| `confirm` | `to: Requested` becomes `to: Active` | killed | `transition/confirm/by/Retry/accepted`, `Retry/outcome/accepted` (2) |
| `relose` | `to: Uncertain` becomes `to: Active` | killed | `transition/relose/by/Retry/lost`, `Retry/outcome/lost` (2) |
| `hold` | `to: Requested` becomes `to: Active` | killed | `ObserveUnlisted/outcome/unconfirmed`, `transition/hold/by/ObserveUnlisted/unconfirmed` (2) |
| `adopt` | drop `Active`, `Requested`, `Uncertain` from `from` (3 mutants) | killed, each | `state/<dropped>/accepts/ObserveListed` (1 each) |
| `adopt` | `to: Active` becomes `to: Cancelled` | stillborn | ESS-MUTATE-002 ESS-ENTITY-011: `Active` cannot be reached from `Declared` |
| `oblige` | drop each of `Active`, `Requested`, `StopRequired`, `Uncertain` (4 mutants) | killed, each | `state/<dropped>/refuses/RequestStop` (1 each) |
| `oblige` | `to: StopRequired` becomes `to: Active` | stillborn | ESS-MUTATE-002 ESS-ENTITY-011: `StopRequired` cannot be reached from `Declared` |
| `discharge` | drop each of `Active`, `Requested`, `StopRequired`, `Uncertain` (4 mutants) | killed, each | `state/<dropped>/refuses/ConfirmStopped` (1 each) |
| `discharge` | `to: Stopped` becomes `to: Active` | stillborn | ESS-MUTATE-002 ESS-ENTITY-011: `Stopped` cannot be reached from `Declared` |
| `disown` | drop each of `Active`, `Requested`, `StopRequired`, `Uncertain` (4 mutants) | killed, each | `state/<dropped>/accepts/ObserveTakenOver` (1 each) |
| `disown` | `to: Disowned` becomes `to: Active` | stillborn | ESS-MUTATE-002 ESS-ENTITY-011: `Disowned` cannot be reached from `Declared` |

Scenario ids are shortened: `transition/…` and `state/…` stand for
`llm-gateway.hosting.OwnedResource/transition/…` and `…/state/…`, and command names drop the
`llm-gateway.hosting.` prefix.

A stillborn mutant is one ESS refuses to emit, because retargeting the only transition into a
state leaves that state unreachable and the mutated specification invalid. Four transitions are
the only route into their target, so their `transition-to` mutant is stillborn and each is scored
through its `from-drop` mutants instead. The other eight `transition-to` mutants are scored
because their targets have more than one route in: `Requested` (`submit`, `confirm`, `hold`),
`Uncertain` (`lose`, `misanswer`, `relose`) and `Cancelled` (`withdraw`, `cancel`), so retargeting
one keeps the state reachable through another.

## The gap this run found, and the workaround

The first run, with one `Observe` command whose `listing` input selected the `adopt`, `disown` and
`hold` moves, reported 27 mutants, 16 killed, 7 survived, 4 stillborn. The seven survivors were
every `from-drop` of `adopt` and `disown`.

ESS 0.77.0 derives the wrong states of a command, not of a move: the entity's states minus the
union of the `from` sets of every move the command declares
(`crates/verify/ess-conformance/src/synthesize.rs:13700-13715`, from `EssIr::wrong_states` at
`crates/specify/ess-compiler/src/ir.rs:3690`, ess source `f692aab`). With one command the wrong
states were `Declared`, `Stopped`, `Cancelled` and `Disowned` only. A `Listed` listing sent to a
`StopRequired` record is a wrong state of the `adopt` move, but no scenario sent it, and each
transition scenario arranges one source state. Dropping a source state from `adopt` or `disown`
therefore changed no scenario's expectation.

`spec/domains/hosting.yaml` works around it with one command per listing (`ObserveListed`,
`ObserveTakenOver`, `ObserveUnlisted`), each with one move and its own
`wrong_state: true, refuses: false` branch, so the wrong states are per move. The declaration is
marked as an ESS 0.77.0 workaround. The run above is on that specification.

## What this does not establish

The mutants are of the specification. A survivor would have said the suite cannot tell the
mutated lifecycle from the declared one; zero survivors says it can, for these two classes. It
does not say that the conformance target is the only reading of the lifecycle, and the other
mutation classes were not run. The target answers through the real `Controller` against the
in-process `FakeProvider`; no provider was called.
