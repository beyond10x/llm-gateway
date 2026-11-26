---
format: aep.planning-md/3
id: story:tests-own-their-clocks
kind: story
status: draft
title: Tests drive the published bounds by a clock they own
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

No test of the workspace waits on the wall clock for a bound the gateway publishes: each such
bound is driven by a clock or a timer the test owns, and the test step's ten slowest tests each
take under 5 s.

## Why

Measured on the build node, 2026-11-26, wave 2026-11-26-w05 at the merge of
story:test-step-within-budget: the test step ran 1353 tests in 26.4 s of wall clock, and its ten
slowest tests took 5.997 to 13.870 s each, most of it waiting out the gateway's own socket and
read timeouts at their published bounds (a trickling client against the read timeout, a silent
TLS handshake against the connect bound, the translated path's bounds). Acceptance 5 of
story:test-step-within-budget asked for these waits to move to a clock the test owns; the unit
left them, because the step is far under its 120 s bound and the move needs seams in the product
for its timers. This story takes that half.

## Acceptance

1. Each test of the step's ten slowest that waits on a published bound reads that bound from a
   clock or timer seam the test drives; the product's own timing is unchanged (the bound's number,
   and a test at the real number kept as one test per bound, under 60 s).
2. The ten slowest tests after, with their times, are in the pull request; each under 5 s apart
   from the one test per bound that 1 keeps.
3. No test is removed or loosened.
