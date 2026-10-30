---
format: aep.planning-md/3
id: review-result:adversary-w06-hosting-lifecycle-pass-2
kind: review-result
status: active
title: 'Adversary pass 2: hosting-lifecycle-declared (w06)'
relations:
- reviews: story:hosting-lifecycle-declared
revision: 1
---
Adversary pass 2 (final) on story:hosting-lifecycle-declared, attacking fix 5d862e5; cases committed as
cde22fd: two scenarios under `contracts/hosting/scenarios/` (`adversary-2-*`), declared in
`contracts/ess-inputs.yaml`, suite regenerated.

```
unit: U2 story:hosting-lifecycle-declared, fix 5d862e5, adversary commit cde22fd
verdict: NEEDS-CHANGE
cases: executed 385→387, red 2
origin: introduced 5 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 scratch dir (adv2/)
needs-coordinator: whether the edit of the pass-1 evidence scenario was authorised (answered below)
```

Red: `adversary-2-a-deployment-identifier-holding-a-space-is-admitted-as-input` (ESS 0.77.0 accepts an authored
`id: d 1` that breaks the declared `DeploymentId` type; the target errors) and
`adversary-2-empty-stop-evidence-takes-the-branch-the-guards-select` (the `invalid-identifier` guard matches a space
only, the adapter refuses everything `Identifier::new` refuses, so empty evidence gets a refusal whose guard is
false). Suite: conformance check 387 total, 385 passed, 1 failed, 1 error. EXIT=1.

Not broken: `list_own` makes no scenario pass vacuously; `Identifier` and `PrintableText` equal the Rust alphabets
and bounds byte for byte; `stale-epoch` is reachable only in `Disowned`, as declared; the 31 acknowledgements
are exactly the ids `ess verify diff` reports; the unchanged-record check covers all 10 fields; the
`expect_no_event` drop is suite/53's compact spelling, not lost assertions.

Answer to needs-coordinator: the edit of the pass-1 evidence scenario's expected outcome was authorised by the
wave coordinator in correction round 1 (the controller's stop evidence is `Option<Identifier>`), on condition
that its description names the story; it does. This was the last adversary pass; the coordinator verifies
the correction. The `crates/llm-provision` note (controller.rs:850) and pass 1's finding 5 go to a draft story.

```findings
[
{"file":"docs/hosting.md","line":149,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the claim that typed identifiers stop a scenario sending one the controller refuses is false: ess 0.77.0 admits id 'd 1' and the target errors at checks/conformance/src/hosting.rs:985"},
{"file":"spec/domains/hosting.yaml","line":657,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"invalid-identifier is guarded on a space only, while the adapter refuses all that Identifier::new refuses, so empty evidence (admitted by ess) gets a refusal whose declared guard is false"},
{"file":"contracts/hosting/scenarios/a-stop-confirmed-with-evidence-holding-a-space-is-discharged.yaml","line":3,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the pass-1 scenario's expected outcome was rewritten on a coordinator decision; its id still says is-discharged while it now asserts a refusal"},
{"file":"spec/domains/hosting.yaml","line":514,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"Declare no longer sets deployment_spec_id, so the newly published view field is asserted by nothing and a wrong value stays green"},
{"file":"crates/llm-provision/src/controller.rs","line":850,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"ObserveListed answered unchanged in StopRequired still writes the observed identity, so resource_name and resource_incarnation change under an outcome named unchanged; exposed by the lifecycle, code unchanged by the unit"}
]
```
