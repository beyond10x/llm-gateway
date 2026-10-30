---
format: aep.planning-md/3
id: approval-record:openai-hosted-qualification-no-runpod
kind: approval-record
status: draft
title: Live qualification runs against OpenAI's hosted API with a connectors-held key; Runpod is not used
relations:
- decides: story:client-qualification
- decides: story:loom-qualification
- decides: story:hosted-credential-from-connectors
revision: 1
---
## Decision

Timo, 2026-10-29, in chat, option A:

- Runpod will not be used for live qualification. The Runpod account stays unfunded.
- Live qualification runs against OpenAI's hosted API through the gateway, with an OpenAI API key
  Timo stores in connectors. Custody is the Runpod key's: the gateway never holds a key file for it.
- The $15 ceiling `decision-blocker:qualification-spend` cleared on 2026-10-08 stays, and now
  applies to OpenAI usage for the live runs of `story:client-qualification` and
  `story:loom-qualification` together.

## What it replaces

The qualification target was a model on a real Runpod pod, started on demand by the binary
(`story:client-qualification`, `story:loom-qualification`), and the stories waited on
`third-party-blocker:runpod-account-unfunded`. Both stories now name an OpenAI model on a hosted
endpoint (`[hosted.<alias>]`, `story:hosted-endpoints`). They no longer measure vLLM's behaviour or
a cold start.

## What it requires first

Today a hosted endpoint reads its key from a `0600` file (`credential_file`, `credential set`;
`README.md` § hosted endpoints); only the Runpod key comes from connectors. The new draft
`story:hosted-credential-from-connectors` lets a hosted endpoint take its credential from a
connectors connection handed over on descriptor 3 (`connectors connections launch --consumer …`).
connectors 0.59.1 ships no OpenAI api-key connector (`connectors list`), recorded as
`upstream-blocker:connectors-openai-connector`. Nothing is written in the connectors repository from
here.

## Store changes made with this record

- `story:client-qualification`, `story:loom-qualification`: retitled and rewritten for the OpenAI
  hosted target; the Runpod-only `depends_on` edges to `story:live-runpod-wiring`,
  `story:pod-proxy-tls` and `story:cold-start-hold` removed from `story:client-qualification`; it
  now depends on `story:hosted-credential-from-connectors`.
- `third-party-blocker:runpod-account-unfunded`: its `blocks` edges removed, moved to `cleared` with
  a section recording this decision.
- `epic:client-access`: retitled and its outcome restated for a hosted model.

## Candidates for the operator, not moved

Runpod-specific artifacts this decision may affect. None is moved or archived here; each is a
separate call.

- `epic:hosting` (draft): provisioned inference, Runpod its only implemented adapter.
- `story:in-pod-connectors` (draft) and `upstream-blocker:connectors-container-keyring` (open): a
  gateway in a Kubernetes pod reaching the Runpod control plane.
- `story:orphan-termination-obligations` (draft): Runpod pod stop obligations.
- `story:hosting-lifecycle-declared` (proposed): the hosting ownership state machine.
- `story:modal-hosting` (draft): a second provisioned-hosting adapter.
- Implemented Runpod stories whose code stays: `story:live-runpod-wiring`, `story:cold-start-hold`,
  `story:pod-proxy-tls`, `story:runpod-production-transport`, `story:runpod-hosting`,
  `story:spend-caps`, `story:model-profile-fallback`, `story:model-wake-and-status`.
- `architecture-decision-record:on-demand-gpu-lifecycle` (accepted): the gateway owns on-demand GPU
  lifecycle; unchanged, but no live run now exercises it.
- `decision-blocker:qualification-spend` (cleared): its body still prices Runpod cards; the ceiling
  is carried over by this record, not by an edit there.
