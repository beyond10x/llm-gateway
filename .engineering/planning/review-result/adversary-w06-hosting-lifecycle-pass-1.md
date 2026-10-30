---
format: aep.planning-md/3
id: review-result:adversary-w06-hosting-lifecycle-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: hosting-lifecycle-declared (w06)'
relations:
- reviews: story:hosting-lifecycle-declared
revision: 1
---
Adversary pass 1 on story:hosting-lifecycle-declared (impl/hosting-lifecycle-declared at ad3e65b; cases
committed as 830e02b: three scenarios under `contracts/hosting/scenarios/`, declared in
`contracts/ess-inputs.yaml`, suite regenerated).

```
unit: U2 story:hosting-lifecycle-declared, branch impl/hosting-lifecycle-declared at ad3e65b (adversary commit 830e02b on top)
verdict: NEEDS-CHANGE
cases: executed 373→376, red 3
origin: introduced 4 / pre-existing 0 / undecided 1
wrote-outside-worktree: 1 scratch dir (adv1/)
needs-coordinator: whether an Uncertain record reached by misanswer is adoptable (findings 1 and 2)
```

Red: `a-create-accepted-outside-the-scope-is-adopted-by-a-listing-that-carries-it` and
`a-create-accepted-outside-the-scope-is-disowned-by-a-listing-retagged-at-a-newer-epoch` (no declared outcome
reached; the record is discharged `Stopped` with `inventory-complete-absent`, because the adapter's listing
after a misanswer does not carry the resource), and `a-stop-confirmed-with-evidence-holding-a-space-is-discharged`
(target error: the adapter requires a hosting identifier for a field the spec types as a plain String).
Suite: conformance check 376 total, 373 passed, 2 failed, 1 error. EXIT=1.

Not broken: the lifecycle-equals-`TRANSITIONS` test compares both directions and the length; every mutation
goes through `Controller::apply`; the view's `state` is the only phase; the 27 acknowledgements are exactly
the unit's breaking changes; the four stillborn mutants re-emit with the codes the record quotes; the unit's
two scenarios assert state and evidence; CHANGELOG and docs match the code.

Coordinator decisions: findings 1 and 2 are fixed in the adapter, not the spec. `ObserveListed` and
`ObserveTakenOver` mean a listing that carries (or retags) the record's resource, whatever route reached
`Uncertain`. Finding 4: the modelled takeover advances the lease registry and the spec declares the refusal
the controller answers. Finding 5 lies in `crates/llm-provision`, which the unit did not change; it is left
for a later story.

```findings
[
{"file":"checks/conformance/src/hosting.rs","line":950,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"ObserveListed on an Uncertain record reached by misanswer reaches no declared outcome and discharges the record, though the lifecycle declares adopt from Uncertain"},
{"file":"checks/conformance/src/hosting.rs","line":951,"category":"contract-drift","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"ObserveTakenOver after a misanswer retags nothing and discharges the record, though the lifecycle declares disown from Uncertain"},
{"file":"checks/conformance/src/hosting.rs","line":980,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"lifecycle command inputs declared String error in the target unless they are hosting identifiers, and the spec declares no refusal for them"},
{"file":"checks/conformance/src/hosting.rs","line":952,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the modelled takeover never advances the lease registry, so the declared wrong-phase for Provision and Retry on Disowned hides the stale-epoch a real takeover answers"},
{"file":"crates/llm-provision/src/controller.rs","line":750,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"undecided","message":"an ownership-lost Uncertain record is discharged as inventory-complete-absent by a complete listing of its own account, contrary to settle_create's open-obligation comment"}
]
```
