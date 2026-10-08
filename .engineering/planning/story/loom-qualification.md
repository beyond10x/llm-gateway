---
format: aep.planning-md/3
id: story:loom-qualification
kind: story
status: draft
title: A Loom run completes a recorded live turn against the same pod through llm
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:client-qualification
scope:
- confidence: cited
  path: docs/verification/loom-qualification.md
revision: 3
---
## Outcome

One live Loom run, configured as `docs/design/runpod-clients.md` § 5 step 7 says, completes a turn
in which the model calls a tool and receives its result, against the model the Claude Code and
Codex sessions of `story:client-qualification` used, through the binary, on the `chat` wire.

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
3. The run's Runpod charge is recorded against the ceiling `decision-blocker:qualification-spend`
   approved.
4. Every other failure it records is either fixed by a story it names or listed as an open finding
   in the record.

## Depends on

`story:client-qualification`: the same binary, model and capture method, already qualified for
two clients. Blocked by `dependency-blocker:llm-model-factory` and
`dependency-blocker:loom-route-option`.
