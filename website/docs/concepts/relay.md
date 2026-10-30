---
title: The relay
sidebar_position: 2
description: How the owner's Chat Completions, Responses and Messages requests reach a model target, what is refused first, and what the binary does with them today.
lede: A wire request is admitted, checked against its model, and only then handed to a target the embedding chose; a refused request never wakes a pod.
source: docs/gateway.md (The relay), crates/llm-gateway/src/relay.rs, crates/llm-gateway/tests/wire_relay.rs, crates/llm-gateway-cli/src/serve.rs, crates/llm-gateway-cli/src/hosted.rs, crates/llm-gateway-cli/tests/relaying.rs, crates/llm-gateway-cli/tests/hosted.rs
---

# The relay

The gateway library composes with a `Relay`: the relayed models and an injected `RelayTargets`.
A `RelayModel` is an alias, the model name its target expects, a non-empty set of distinct wires
and its tool calling, `Parsed` or `Absent`. The embedding implements `RelayTargets` over its own
pool: `acquire` hands out the target serving an alias, and `invalidate` is told when a request
through one failed. A target opens its own connection, so the transport, TLS included, belongs to
the embedding; the gateway crate opens none.

| Wire | Path | Client that speaks it |
| --- | --- | --- |
| `chat` | `POST /v1/chat/completions` | Loom, through an llm catalog |
| `responses` | `POST /v1/responses` | Codex |
| `messages` | `POST /v1/messages` | Claude Code |

The binary's targets are Runpod pods and hosted endpoints. A hosted endpoint is any server that
speaks the model's wires at the `http` or `https` base URL its `[hosted.<alias>]` table declares
([the deployment document](../reference/deployment-document.md#hosted-models)); it is asked on the
same path, with a bearer, an API key in the header the table names, or no credential, and an
`https` one is verified as a pod is. A credential is never sent in cleartext to another machine:
an endpoint with a credential at an `http` base URL must be on a loopback host. A pod is reached at its `https` proxy URL,
`https://<pod id>-8000.proxy.runpod.net/v1/`, over TLS: the binary's client verifies the pod's
certificate, chain and host name, against the Mozilla roots compiled into the binary before it
writes any byte of the request, and a certificate that does not verify is `upstream-failed`. No
document key or option adds a root or turns verification off. A model whose provider declares no
`connectors` table is answered `target-unavailable`.

:::caution[Not run against Runpod]
The relay to a pod is tested end to end against a fixture of the `connectors` CLI and a loopback
`https` pod whose certificate a test root issued, never against Runpod. The relay to a hosted
endpoint is tested against loopback endpoints only, never against a vendor API.
:::

## The order a wire request is decided in

After the head, the probes, shedding and owner authentication of
[the single-owner gateway](single-owner-gateway.md):

1. **`POST` only** (`method-not-allowed`, `allow: POST`), and `unavailable` before readiness.
2. **The body**: `content-length` or `transfer-encoding: chunked`, at most 32 MiB. A declared
   length over the bound is `body-too-large` before a byte is read; a chunked body is refused the
   moment it passes it. A body that ends or stalls is `body-incomplete`.
3. **One JSON object** (`body-not-json`) with exactly one top-level `model` string
   (`model-absent`), naming a relayed model (`model-unknown`).
4. **The wire**: a request on a wire the model declares is relayed as below. The binary
   translates a request on any other wire within llm's published subset
   ([Translate between wires](../guides/translate-between-wires.md)); a gateway composed without a
   translator refuses it `wire-not-served`.
5. **Tools**: a non-empty top-level `tools` array for a model whose tool calling is `Absent` is
   `tools-not-served`. A pod started without a tool-call parser could not answer it.
6. **The target**: only now is a target asked for. `target-unavailable` when there is none;
   `model-cold-start` with `retry-after: 30` when the model's pod is still starting after the
   request's hold budget.

7. **The output clamp**, on the Messages wire only. A model's window is its `max_model_len`
   when it declares one, else its `context_window`: the binary declares a Runpod model's
   `max_model_len` and a hosted model's `context_window`, so a hosted model without one has no
   window. For a model with a window and a body whose top-level `max_tokens` is one
   non-negative integer, the gateway first asks the same target
   `POST /v1/messages/count_tokens` with the body it is about to relay and the same credential.
   A `max_tokens` above the room the counted input leaves is lowered to that room; an input that
   leaves no room is `context-exceeded` (400), whose message starts `prompt is too long`, the
   phrase Claude Code compacts its conversation on. A count that fails relays the request
   unchanged, and so does one not answered within ten seconds of its connection opening (the
   embedding may set another deadline): a target that never answers the count holds the
   request no longer than that.

Nothing before step 6 asks for a target, so a refused request never wakes a pod, and the count
of step 7 asks only the target step 6 handed out.

## What reaches the target

- The top-level `model` value becomes the target's model name. On the Messages wire,
  `output_config.effort` and `chat_template_kwargs.reasoning_effort` equal to `high` or `max`
  become `xhigh`, the deepest effort the served Qwen3.8 chat template accepts; `low`, `medium`
  and `xhigh` pass unchanged. A Messages request that names no effort, sent to a hosted model
  that declares a `reasoning_effort`, gains `output_config.effort` set to it. Every other byte
  reaches the target unchanged, Claude Code's
  adaptive `thinking` and its structured-output `output_config.format` included: the pinned
  vLLM ignores the first and serves the second as structured output.
- The head is the gateway's own: `host` is the target's authority, and a target with a bearer is
  sent `authorization: Bearer <key>` (for a Runpod pod, the model's vLLM key). An `api-key`
  hosted endpoint is sent its credential in the header its table names instead, and no
  `authorization`. No header of the client's request is forwarded, so the owner's credential
  never reaches a target; the one exception is a valid `x-opencode-session` to an OpenCode Go
  endpoint ([the deployment document](../reference/deployment-document.md#hosted-models)).
  Claude Code's `anthropic-version` and `anthropic-beta` are dropped with the rest; the pinned vLLM reads neither. A Bedrock model, which is translated and not
  relayed, receives only the beta names its `anthropic_beta` list allows
  ([beta names](../guides/serve-a-bedrock-model.md#beta-names)).
- The target's status, `content-type` and body are relayed as they arrive, chunked, so a stream
  reaches the client event by event. The body is not rewritten: whatever a target puts in its
  answer body reaches the client. An answer cut short reaches the client cut.

## When a target fails

A connection that fails, an answer head that never arrives or is malformed, and a `502`, `503` or
`504` each tell `invalidate` which endpoint served the request and answer `upstream-failed`
(`502`); the next request asks for a replacement. A hosted endpoint has no replacement, so it is
kept and the next request asks it again. Any other status, a `500` or a `4xx` included, is the
model's own answer and is relayed with its body.

Fallback to another target on the same request is planned, not built.

## Cold starts

The embedding holds a request while its target starts and owns the hold budget. The Runpod
composition asks its pool every 500 ms for up to the model's `request_hold_seconds`, which
defaults to `start_wait_seconds` (600). Past the budget the request is `model-cold-start`, the
only refusal that carries `retry-after`. Requests held together wait on one pod. See
[hosting](hosting.md).

## Each request is recorded

Every wire request that passed authentication makes one usage record once its answer or refusal
is written: the model, the wire, whether it was relayed, refused or failed upstream, the status,
the response bytes and the duration. Token counts are always absent for now; carrying the counts
the target reported is planned. [Scrape the counters](../guides/scrape-the-counters.md) shows the
counters the records feed.
