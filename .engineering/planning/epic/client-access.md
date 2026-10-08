---
format: aep.planning-md/3
id: epic:client-access
kind: epic
status: draft
title: Claude Code, Codex and Loom use a Runpod-hosted model through llm-gateway
relations:
- serves: vision:portable-model-inference
revision: 2
---
## Outcome

The operator runs Claude Code, Codex and a Loom run against a model served from a Runpod pod,
through one `b10x-llm-gateway` on the workstation and one owner token, with the pod started on
demand and terminated when idle.

## Why

Operator request, 2026-10-08: "draft a design which would allow me to use a runpod hosted model
within claude, codex and loom".

## Design

`docs/design/runpod-clients.md`. The nouns are declared `PLANNED` in `spec/domains/clients.yaml`
(`llm-gateway.clients`): `ClientKind`, `ClientProfile`, `ToolCalling`, `ClientRefusal`,
`ThinkingHandling`. `ess specify validate --path spec` reports the system valid on ESS 0.56.0 and
0.52.0.

## What this epic adds, and what it relies on

The relay chain is already planned in `epic:gateway` and `epic:hosting`:
`story:provider-key-files`, `story:gateway-deployment`, `story:runpod-production-transport`,
`story:live-runpod-wiring`, `story:cold-start-hold` and `story:public-model-listing`. This epic
adds only what the three clients need beyond that chain:

| Story | What it adds |
| --- | --- |
| `story:model-tool-calling` | a model declares tool calling; a request with tools for a model without it is refused before any pod starts |
| `story:pod-proxy-tls` | the binary reaches a pod's `https` proxy URL over verified TLS |
| `story:client-qualification` | one recorded live session each for Claude Code and Codex against a real pod, with its cost |
| `story:claude-code-wire` | the gateway answers what Claude Code sends, settled from the qualification record |
| `story:loom-qualification` | one recorded live Loom run through llm against the same pod |

`story:public-model-listing` (in `epic:gateway`) renders the `ClientProfile` settings at `GET /`.

## Not in scope

- Protocol translation between wires (`story:gateway-translation`). Each client is served on its
  own wire.
- A gateway reachable from another machine (design choice D4): it listens on loopback.
- Changes in beyond10x/llm, beyond10x/loom or beyond10x/connectors. They are requested there, and
  this store holds them as `dependency-blocker`s.

## Done when

Each of the five stories is `implemented` or `archived`, an archived one giving its reason in its
body. `docs/verification/client-qualification.md` shows, for Claude Code and for Codex, an answer
carrying a tool call and a following request carrying its result. `docs/verification/loom-qualification.md`
shows the same for Loom, unless `story:loom-qualification` is archived, in which case its body names
why and the epic is done without the Loom record.
