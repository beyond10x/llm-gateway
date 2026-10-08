---
format: aep.planning-md/3
id: story:client-qualification
kind: story
status: draft
title: Claude Code and Codex each complete a recorded live session against a Runpod pod
relations:
- decomposes: epic:client-access
- serves: vision:portable-model-inference
- depends_on: story:live-runpod-wiring
- depends_on: story:pod-proxy-tls
- depends_on: story:cold-start-hold
- depends_on: story:model-tool-calling
- depends_on: story:gateway-observability
scope:
- confidence: cited
  path: docs/verification/client-qualification.md
- confidence: cited
  path: docs/verification/fixtures/claude-code-2.1.293-messages.json
revision: 5
---
## Outcome

One live session each of Claude Code 2.1.293 and Codex 0.160.0, configured as
`docs/design/runpod-clients.md` § 5 says, runs a task that asks for a tool call against a model on a
real Runpod pod through the binary. The record names what each client sent, what vLLM answered,
whether the tool call and its result went through, how much of the context window a session used,
the cold-start time and what the run cost. A failure is a finding the record assigns to a story;
fixing it is that story's acceptance, not this one's.

## Why

Facts the design cannot settle without a live pod: whether vLLM's `/v1/messages` accepts Claude
Code's `thinking: {"type":"adaptive"}` (design choice D3), whether vLLM's `/v1/responses` accepts
Codex's `include: ["reasoning.encrypted_content"]` and free-form `apply_patch`, which of llmgw's
context windows (32768 or 65536 tokens) leaves the clients room (design § 4), and how long a cold
start takes (the design's cost table says "I don't know").

## Method

Each client talks to the binary through a loopback capture between the two (plain HTTP on
`127.0.0.1`), which records every request and answer byte with a UTC time for each request and for
the first byte of each answer; the `authorization` value is removed from the record. Each call's
duration is cross-checked against the per-call record `story:gateway-observability` adds
(`llm-gateway.telemetry.UsageRecord`, `duration_ms`).

## Acceptance

1. `docs/verification/client-qualification.md` records, per client: the settings used (no secret
   value), the request lines and every non-2xx answer with its body from the capture, and the
   session's outcome.
2. Per client, the record states, from the capture, whether an answer carried a tool call and
   whether a following request carried its result. Where either is missing, the record names the
   cause and the story that owns the fix: `story:claude-code-wire` for Claude Code, and for Codex a
   story that does not depend on this one (`story:gateway-translation` when vLLM refuses a field
   Codex sends).
3. The first inference request Claude Code sent is committed, with the `authorization` value
   removed, as `docs/verification/fixtures/claude-code-2.1.293-messages.json`.
4. Per client, the record states the `context_window` of the model used and the largest
   `usage.input_tokens` any answer in the session reported, read from the capture.
5. The record gives the cold-start time, from the first request to the first byte of its answer
   once the pod served, for at least two starts, with the capture's UTC times, and the matching
   `duration_ms` of the per-call record.
6. The record gives the Runpod charge for the run, read from the Runpod console's billing page,
   against the ceiling `decision-blocker:qualification-spend` approved.
7. Every other refusal or failure it records names the story that owns the fix, or is listed as an
   open finding in the record.
8. No test in `task check` depends on it: the gate makes no paid call (`AGENTS.md` "Invariants").

## Depends on

`story:live-runpod-wiring`, `story:pod-proxy-tls`, `story:cold-start-hold` and
`story:model-tool-calling`: together they are a binary that starts a real pod, reaches it over TLS,
holds a cold request and refuses a tool request to a model that cannot answer it.
`story:gateway-observability`: the per-call records criterion 5 reads. Blocked by
`decision-blocker:qualification-spend`: a live run spends money.
