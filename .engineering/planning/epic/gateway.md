---
format: aep.planning-md/3
id: epic:gateway
kind: epic
status: draft
title: 'Gateway: the serving half of llm''s gateway and release qualification'
revision: 1
---
## Outcome

Gateway and release qualification supplies its part of the full foundation described in docs/design.md.

## Done when

All child outcomes have retained verification evidence and their contracts are included in story:foundation-qualified; the existing compile-only scaffold is not completion evidence.

## Evidence

Operator direction 2026-09-19; docs/design.md; spec/system.yaml.

## Moved

Moved from `beyond10x/llm` (`epic:gateway`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/epic/gateway/`. Only its gateway half moved: the stories here; llm keeps epic:gateway for the rest.
