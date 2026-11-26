---
format: aep.planning-md/3
id: story:spend-cap-counts-earlier-pods
kind: story
status: active
title: The spend ceilings count this gateway's stopped pods and its pods from before a restart
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T17:40:38Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-26T17:40:38Z", actor: "human:operator", revision: 4}
---
## Outcome

The daily and total spend ceilings count what this gateway's own pods cost today, including pods
already stopped and pods started before the gateway restarted, until Runpod's billing reports them.

## Why

Measured on the owner's host, 2026-11-26, `daily_cap_usd = 3.33`. Between 15:09Z and 16:34Z the
gateway ran five pods (2.69, 2.69, 4.39, 0.59 and 1.09 USD/h); their cost times their run time is
about 2.4 USD. At 16:33Z `GET /v2/billing/pods?bucketSize=day` reported 0.87 USD for the day in
two records. The spend check adds the reported spend, an estimate of the pool's live pods and an
estimate of the pod about to start (`crates/llm-runpod/src/spend.rs`). A stopped pod leaves the
estimate, and a restart forgets every earlier pod, so a wake was admitted against about 0.87 USD
of the 2.4 USD spent. The gateway was restarted six times that day.

## Acceptance

- The pool keeps a ledger of its own pods' estimated spend per UTC day (price per hour times run
  time, from submission to confirmed stop) that survives a restart, in a file under the gateway's
  state directory.
- The spend check uses, for the day, the larger of the reported spend and the ledger, plus the
  live and about-to-start estimates as now; for the total ceiling the same rule over the reported
  window.
- Tests: a stopped pod still counts until billing reports at least as much; a restarted pool
  reads the ledger and refuses a start that the ledger puts over the ceiling; a ledger that
  cannot be read refuses as `unknown`.
- `docs/hosting.md` and the deployment-document reference say where the ledger lives and what
  it counts.
