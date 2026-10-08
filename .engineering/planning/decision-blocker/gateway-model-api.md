---
format: aep.planning-md/3
id: decision-blocker:gateway-model-api
kind: decision-blocker
status: cleared
title: Are models and vLLM parameters set through an owner API on the running gateway? (design choice D5)
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T00:54:33Z", actor: "human:timo", revision: 2}
---
## Question

Design choice D5 of `docs/design/runpod-clients.md` § 7: are models and vLLM parameters set
through an owner-authenticated API on the running gateway, and when?

| Option | What it does | What it costs |
| --- | --- | --- |
| A | A later epic, after `story:client-qualification`. `epic:client-access` works on the static `[models.<alias>]` tables of the deployment document. | Changing a model or its vLLM arguments needs a document edit and a gateway restart until the later epic lands. |
| B | Now: models become commands on the running gateway, replacing the document's `[models]` before the relay chain is built. | The relay chain waits on durable model records, swapped route-inventory snapshots and the spending ceilings in the document. |

Recommendation at filing: A. The relay chain and the clients work on the static document first,
and § 7 needs the spending ceilings in the document either way.

## Cleared when

The operator picks A or B, and the choice is written under `## Decision` here and in the D5 row
and § 7 of `docs/design/runpod-clients.md`.

## Decision

A, the operator, 2026-10-08. The owner API for models and vLLM parameters is a later epic, drafted
after `story:client-qualification` is implemented. `epic:client-access` keeps the model registry in
the deployment document.
