---
title: The relay
sidebar_position: 2
description: How the owner's Chat Completions, Responses and Messages requests reach a model target, what is refused first, and what the binary does with them today.
lede: A wire request is admitted, checked against its model, and only then handed to a target the embedding chose; a refused request never wakes a pod.
source: docs/gateway.md (The relay), crates/llm-gateway/src/relay.rs, crates/llm-gateway/tests/wire_relay.rs, crates/llm-gateway-cli/src/serve.rs, crates/llm-gateway-cli/tests/relaying.rs
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

:::caution[Planned: the binary relays to no pod yet]
The library relays, and the binary composes the relay, but the binary has no production target
yet. A model whose provider declares no `connectors` table is answered `target-unavailable`. A
document whose provider declares one is refused at startup as `config:value`, because a pod is
reached at its `https` proxy URL and the binary does not speak TLS to it yet. The Runpod pool
behind the relay is tested through a seam in the binary's own tests.
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
4. **The wire**: the model must declare the path's wire (`wire-not-served`).
5. **Tools**: a non-empty top-level `tools` array for a model whose tool calling is `Absent` is
   `tools-not-served`. A pod started without a tool-call parser could not answer it.
6. **The target**: only now is a target asked for. `target-unavailable` when there is none;
   `model-cold-start` with `retry-after: 30` when the model's pod is still starting after the
   request's hold budget.

Nothing before step 6 asks for a target, so a refused request never wakes a pod.

## What reaches the target

- The top-level `model` value becomes the target's model name. On the Messages wire,
  `output_config.effort` and `chat_template_kwargs.reasoning_effort` equal to `high` become
  `xhigh`. Every other byte reaches the target unchanged.
- The head is the gateway's own: `host` is the target's authority, and a target with a bearer is
  sent `authorization: Bearer <key>` (for a Runpod pod, the model's vLLM key). No header of the
  client's request is forwarded, so the owner's credential never reaches a target.
- The target's status, `content-type` and body are relayed as they arrive, chunked, so a stream
  reaches the client event by event. An answer cut short reaches the client cut.

## When a target fails

A connection that fails, an answer head that never arrives or is malformed, and a `502`, `503` or
`504` each tell `invalidate` which endpoint served the request and answer `upstream-failed`
(`502`); the next request asks for a replacement. Any other status, a `500` or a `4xx` included,
is the model's own answer and is relayed with its body.

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
