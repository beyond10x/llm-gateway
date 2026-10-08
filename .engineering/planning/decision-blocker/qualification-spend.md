---
format: aep.planning-md/3
id: decision-blocker:qualification-spend
kind: decision-blocker
status: cleared
title: No spending ceiling is approved for the live qualification runs
relations:
- blocks: story:client-qualification
- blocks: story:loom-qualification
withholds: test_result
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T10:03:41Z", actor: "human:timo", revision: 3}
---
## Question

How much may a live qualification run spend on Runpod? `story:client-qualification` and
`story:loom-qualification` each start a real pod. Spending is the operator's decision.

## Estimate

From `docs/design/runpod-clients.md` § 6: an L40S on Secure Cloud is $1.09/hour, and a cold start is
at most 1800 s. Two client sessions of one hour each, two cold starts and two 30-minute idle tails
come to about $4.40 on that card; the H100 NVL profile about $12.80. Proposed ceiling: $15 for both
stories together.

## Decided 2026-10-08: $15

The operator approved a ceiling of $15 in total for the live runs of `story:client-qualification`
and `story:loom-qualification` together, on either profile above. A run that would pass the ceiling
stops before it does and reports; it is not continued on the strength of an estimate.

## Cleared when

The operator approves a ceiling in dollars (done above). The qualification records cite it and
state what each run spent.
