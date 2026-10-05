---
format: aep.planning-md/3
id: story:runpod-production-transport
kind: story
status: draft
title: Runpod has a production REST/GraphQL transport
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 1
---
## Acceptance

Filed from wave 3 (2026-09-27) to own a `DEFERRED:` note in `spec/domains/runpod.yaml`. The
acceptance is written when the story is scoped.

## Moved

Moved from `beyond10x/llm` (`story:runpod-production-transport`, llm `8d8e752d`) by story:serving-extraction on
2026-10-05, with the crates it describes. Its lifecycle history and evidence records stay in
llm's store at `.engineering/evidence/story/runpod-production-transport/`. Its dependencies on stories that stay in llm (named in its llm record) are on llm's released client crates.
