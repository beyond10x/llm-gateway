---
format: aep.planning-md/3
id: epic:hosting
kind: epic
status: draft
title: Provisioned inference
revision: 3
---
## Outcome

Provisioned inference supplies its part of the full foundation described in llm's `docs/design.md` (https://github.com/beyond10x/llm at `8d8e752d`); this repository's part is the hosting contract in `docs/hosting.md`.

## Done when

All child outcomes have retained verification evidence and their contracts are included in story:foundation-qualified; the existing compile-only scaffold is not completion evidence.

## Evidence

Operator direction 2026-09-19. The design it cites is llm's: `docs/design.md` and `spec/system.yaml` in https://github.com/beyond10x/llm at `8d8e752d`. In this repository the hosting contract is `docs/hosting.md`, the system is `spec/system.yaml` and the hosting domain is `spec/domains/hosting.yaml`.

## Moved

Moved from `beyond10x/llm` (`epic:hosting`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/epic/hosting/`.
