---
format: aep.planning-md/3
id: story:client-qualification
kind: story
status: draft
title: Claude Code and Codex each complete a recorded live session against OpenAI's hosted API through the gateway
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:model-tool-calling
- depends_on: story:gateway-observability
- depends_on: story:claude-code-wire
- depends_on: story:hosted-credential-from-connectors
scope:
- confidence: cited
  path: docs/verification/client-qualification.md
- confidence: cited
  path: docs/verification/fixtures/claude-code-2.1.293-messages.json
revision: 9
---
## Outcome

One live session each of Claude Code 2.1.293 and Codex 0.160.0, configured as
`docs/design/runpod-clients.md` § 5 says for the client side, runs a task that asks for a tool call
against an OpenAI model served from OpenAI's hosted API through the binary, as a `[hosted.<alias>]`
endpoint whose credential comes from a connectors connection
(`story:hosted-credential-from-connectors`). The record names what each client sent, what the
endpoint answered, whether the tool call and its result went through, how much of the context window
a session used, the time to first byte and what the run cost. A failure is a finding the record
assigns to a story; fixing it is that story's acceptance, not this one's.

## Retargeted 2026-10-29

Operator decision `approval-record:openai-hosted-qualification-no-runpod`: Runpod is not used and
its account stays unfunded. This story ran against a Runpod pod until then. It no longer measures
vLLM's behaviour (whether vLLM's `/v1/messages` accepts `thinking: {"type":"adaptive"}`, whether
vLLM's `/v1/responses` accepts Codex's fields) or a cold start; nothing here starts a pod.

## Why

Facts the design and the fixtures cannot settle without a live endpoint: whether a Claude Code
session, translated by the gateway onto an OpenAI wire, completes a tool round trip; whether
OpenAI's `/v1/responses` accepts what Codex sends through the relay (`include:
["reasoning.encrypted_content"]`, free-form `apply_patch`); how much of the model's context window
each client uses; and what a session costs.

## Method

Each client talks to the binary through a loopback capture between the two (plain HTTP on
`127.0.0.1`), which records every request and answer byte with a UTC time for each request and for
the first byte of each answer; the `authorization` value is removed from the record. Each call's
duration is cross-checked against the per-call record `story:gateway-observability` adds
(`llm-gateway.telemetry.UsageRecord`, `duration_ms`). The binary runs under
`connectors connections launch` with the OpenAI connection; no OpenAI key is in a file, the record
or the capture.

## Acceptance

1. `docs/verification/client-qualification.md` records, per client: the settings used (no secret
   value), the OpenAI model and wire the alias reaches, the request lines and every non-2xx answer
   with its body from the capture, and the session's outcome.
2. Per client, the record states, from the capture, whether an answer carried a tool call and
   whether a following request carried its result. Where either is missing, the record names the
   cause and the story that owns the fix: `story:claude-code-wire` for Claude Code, and for Codex a
   story that does not depend on this one (`story:gateway-translation` when the endpoint refuses a
   field Codex sends).
3. The first inference request Claude Code sent is committed, with the `authorization` value
   removed, as `docs/verification/fixtures/claude-code-2.1.293-messages.json`.
4. Per client, the record states the `context_window` of the model used and the largest
   `usage.input_tokens` any answer in the session reported, read from the capture.
5. The record gives the time from request to the first byte of its answer for at least two
   requests per client, with the capture's UTC times, and the matching `duration_ms` of the per-call
   record.
6. The record gives the OpenAI charge for the run, read from OpenAI's usage page for the key's
   project, against the $15 ceiling `decision-blocker:qualification-spend` approved, which
   `approval-record:openai-hosted-qualification-no-runpod` carries over to OpenAI usage.
7. Every other refusal or failure it records names the story that owns the fix, or is listed as an
   open finding in the record.
8. No test in `task check` depends on it: the gate makes no paid call (`AGENTS.md` "Invariants").
9. The Claude Code session checks what `story:claude-code-wire` settled from sources rather than a
   capture, each answered from the capture in the record: whether the first request carries
   `output_config.effort` and at which level, whether an answer with a long thinking pause and no
   `ping` event completes, how an upstream 400 answer is handled, and whether any request carries
   `output_config.format`. A difference from `docs/verification/2026-10-15-claude-code-wire.md`
   names the story that owns the fix.

## Depends on

`story:hosted-credential-from-connectors`: the binary reaches the OpenAI endpoint with the
connectors-held key, which waits on `upstream-blocker:connectors-openai-connector`.
`story:model-tool-calling`: a tool request to a model that cannot answer it is refused.
`story:gateway-observability`: the per-call records criterion 5 reads. `story:claude-code-wire`:
the messages wire it checks. Its spend ceiling is approved: `decision-blocker:qualification-spend`
is cleared with a $15 ceiling for live runs, now applying to OpenAI usage.
