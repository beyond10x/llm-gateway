---
format: aep.planning-md/3
id: story:llmgw-capability-matrix
kind: story
status: implemented
title: llmgw's capabilities are mapped against llm-gateway, row by row
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: docs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-05T10:23:17Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-05T10:23:17Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-05T10:32:45Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"verification":1}}}
---
## Outcome

`docs/llmgw-capability-matrix.md` lists every capability of `beyond10x/llmgw` (routes, backends,
registry, auth, scale-to-zero, configuration, observability, deployment) against `llm-gateway`,
each row citing both sides by file and line, with a status: covered, partial, gap, or not needed
(with the reason). The gap and partial rows are phrased as story titles at the end.

## Why

First acceptance item of llm `story:llmgw-retirement` (operator decision 2026-10-05, "gateway ->
B"): no deployment moves until every llmgw capability has a home or a recorded reason not to.

## Acceptance

- Every public route, CLI command, configuration key and backend of llmgw at its current `main`
  appears as a row; the row count per area is stated and checked against llmgw's own route table,
  CLI help and config schema.
- Each row cites llmgw (`file:line` at a named commit) and llm-gateway (`file:line` at a named
  commit) or says "none".
- The `## Gaps` section has one line per gap or partial row.

## ESS first

None: a documentation record; no behaviour change.

## Not in scope

Closing the gaps; the client runs, the rollback and the archive (llm `story:llmgw-retirement`).
