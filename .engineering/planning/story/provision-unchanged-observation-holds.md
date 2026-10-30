---
format: aep.planning-md/3
id: story:provision-unchanged-observation-holds
kind: story
status: draft
title: An unchanged observation leaves a hosting record unchanged
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
- derived_from: story:hosting-lifecycle-declared
revision: 1
---
## Outcome

Two cases change today, and after this story they do not:

- **An `unchanged` observation.** When the hosting controller answers an observation `unchanged`, the record
  is not changed at all. Today, an `ObserveListed` answered `unchanged` in `StopRequired` still writes
  `record.observed` (`crates/llm-provision/src/controller.rs`, near line 850). So a record that came through
  `Provision Unknown` and then `RequestStop` gets values in `resource_name` and `resource_incarnation`.
- **An ownership-lost `Uncertain` record.** It keeps its obligation open, as the comment on `settle_create`
  says. Today a complete listing of the record's own account discharges it as `inventory-complete-absent`
  (controller.rs, near line 750).

## Why

Adversary passes on story:hosting-lifecycle-declared (wave 2026-10-30-w06) found both:

- `review-result:adversary-w06-hosting-lifecycle-pass-1`, finding 5;
- `review-result:adversary-w06-hosting-lifecycle-pass-2`, finding 5.

The lifecycle makes them visible, but the code is the controller's, and that story did not change it. Each
is either a controller defect or a comment that is wrong. This story decides which, and makes the code, the
comment and the declared lifecycle agree.

## Acceptance

- An authored scenario walks `Provision Unknown` → `RequestStop` → `ObserveListed`. It asserts the outcome
  `unchanged` and that the view shows every field of the record unchanged.
- An authored scenario walks an ownership-lost `Uncertain` record through a complete own-account listing. It
  asserts the outcome the decision names, and `settle_create`'s comment says the same.
- `b10x-llm-gateway-conformance check` passes with both scenarios.
