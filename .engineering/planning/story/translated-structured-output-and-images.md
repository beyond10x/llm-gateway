---
format: aep.planning-md/3
id: story:translated-structured-output-and-images
kind: story
status: draft
title: A translated route carries structured output and a user image where its target can
summary: llm reads both on every client wire since 0.21.0 and 0.22.0; the gateway declares both false on translated paths until this story
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 1
---
## Source

Wave 2026-11-21-w04 moved the llm pin from 0.20.0 to 0.24.0. llm 0.21.0 reads structured
output on every client wire (`response_format`, `text.format`, `output_config.format`) and
sends it on every upstream wire, and llm 0.22.0 does the same for a user image. The gateway
documented both as refused on every translated path, and `GET /` exports
`CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'` for a model Claude Code reaches only translated.
To keep that contract, the wave declares both `false` in every capabilities the translator uses
(`translate::translated`), so llm refuses them before any I/O.

## Outcome

A translated route carries structured output and a user image where llm and the route's target
can carry them, and refuses them by the route's declared capabilities where they cannot. A
routes document's `structured_output` and `image_input` declarations are honoured. `GET /`
stops exporting `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS` for a model whose translated path
carries Claude Code's structured output.

## Acceptance

1. Claude Code's structured-output request reaches a Bedrock `anthropic-invoke` model and a
   chat upstream with its schema, and is answered on the Messages wire.
2. A route declaring `structured_output: false` or `image_input: false` refuses the feature
   `translation-unsupported` before any target is asked for.
3. `docs/gateway.md`, the site and the `GET /` profile say which paths carry each feature.

## Open question

Whether undeclared means carried (llm's rule) or refused (the gateway's rule today) for a route
whose upstream has not been checked against the feature.
