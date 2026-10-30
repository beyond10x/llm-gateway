---
format: aep.planning-md/3
id: story:loom-qualification
kind: story
status: draft
title: A Loom run completes a recorded live turn against the same OpenAI hosted model through llm
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:client-qualification
scope:
- confidence: cited
  path: docs/verification/loom-qualification.md
revision: 6
---
## Outcome

One live Loom run, configured as `docs/design/runpod-clients.md` § 5 step 7 says for the client
side, completes a turn in which the model calls a tool and receives its result, against the OpenAI
hosted model the Claude Code and Codex sessions of `story:client-qualification` used, through the
binary, on the `chat` wire.

## Retargeted 2026-10-29

Operator decision `approval-record:openai-hosted-qualification-no-runpod`: Runpod is not used and
its account stays unfunded. This story ran against the same Runpod pod as
`story:client-qualification` until then; it now runs against the same OpenAI hosted endpoint, and
measures neither vLLM's behaviour nor a cold start.

## Why

Loom reaches models through llm (`docs/design/runpod-clients.md` § 3). llm can declare the route
today, but the `b10x-loom` command line builds only a Codex-subscription model, and llm has no
function that builds a `Model` from a catalog serving model. Both are changes outside this
repository.

## Acceptance

1. `docs/verification/loom-qualification.md` records the llm and loom release tags used, the llm
   catalog (no secret value), and the request lines and answers between Loom and the binary,
   captured on loopback as `story:client-qualification` captures them.
2. The capture shows every inference request on `POST /v1/chat/completions` naming the alias the
   `story:client-qualification` record names, an answer carrying a tool call, and a following
   request carrying its result. A run without them is a failure: the story is not done until a
   story named in the record, here or in the repository that owns it, fixes it and a rerun shows
   them.
3. The run's OpenAI charge, read from OpenAI's usage page for the key's project, is recorded against
   the $15 ceiling `decision-blocker:qualification-spend` approved, shared with
   `story:client-qualification` and carried over to OpenAI usage by
   `approval-record:openai-hosted-qualification-no-runpod`.
4. Every other failure it records is either fixed by a story it names or listed as an open finding
   in the record.

## Depends on

`story:client-qualification`: the same binary, hosted model and capture method, already qualified
for two clients. Blocked by `dependency-blocker:llm-model-factory` and
`dependency-blocker:loom-route-option`.

## Releases that cleared its blockers (2026-10-08)

- `dependency-blocker:llm-model-factory`: llm 0.4.0, https://github.com/beyond10x/llm/releases/tag/0.4.0
  (`story:catalog-model-port`). llm 0.5.0 (https://github.com/beyond10x/llm/releases/tag/0.5.0) is
  the newest and the tag this story takes.
- `dependency-blocker:loom-route-option`: loom 0.7.0, https://github.com/beyond10x/loom/releases/tag/0.7.0
  (the catalog route).

It still waits on `story:client-qualification`, which waits on
`story:hosted-credential-from-connectors` and through it on
`upstream-blocker:connectors-openai-connector`.
