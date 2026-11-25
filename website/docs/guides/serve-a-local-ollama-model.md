---
title: Serve a local Ollama model
sidebar_position: 8
description: Declare a model on an Ollama server as a hosted model, read whether it is pulled and loaded, and know that the gateway never pulls a model.
lede: A hosted model of kind ollama is relayed to the Ollama server's own chat, responses and messages paths; its status comes from the server's lists of pulled and loaded models, and the gateway sends nothing that changes what the server stores.
source: crates/llm-gateway-cli/src/hosted.rs, crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/tests/wake_status.rs (a fake Ollama server on loopback), spec/domains/upstream.yaml (HostedEndpoint) and spec/domains/deployment.yaml (HostedDeclaration); the wire paths were checked on Ollama 0.40.18
---

# Serve a local Ollama model

A `[hosted.<alias>]` table with `kind = "ollama"` declares a model an Ollama server serves.
Clients call the model by its alias on any wire, as they call any other model; the gateway
relays each call to the server's own path for that wire.

## Pull a model with Ollama

The gateway never pulls a model, and never sends a request that changes what the server
stores. Pull the model with Ollama's own command line first:

```sh
ollama pull qwen2.5:1.5b
```

`qwen2.5:1.5b` is a small model to try. Ollama 0.40.18's `GET /api/tags` reported it as
986,061,892 bytes on disk (about 1 GB) on 2026-11-25.

## Declare the model

```toml
[hosted.local]
kind = "ollama"
upstream_model = "qwen2.5:1.5b"
```

That is the whole table. The defaults:

- `base_url` is `http://127.0.0.1:11434`, Ollama's own address. Name another server's root, on
  any host, as `http://<host>:<port>` with no path; the gateway adds `/v1` and `/api` itself.
- `wires` is all three. Each wire named is relayed to Ollama's own path:
  `/v1/chat/completions`, `/v1/responses` and `/v1/messages`. These three paths were checked on
  Ollama 0.40.18. A wire left out of `wires` is translated onto the first one named.
- `upstream_model` is the Ollama model name; it is the alias when left out. A name without a tag,
  such as `llama3.2`, means `llama3.2:latest`.
- `auth_kind` is `anonymous`, and no credential is sent. A credential key or another
  `auth_kind` is refused at start.
- `billing_kind` is `self-hosted`.

The [deployment document reference](../reference/deployment-document.md#ollama-models) lists every
key.

## Read its status

`GET /v1/models/local/status` and `GET /v1/status` read the model's status from the server's
own lists: `GET /api/tags` (the models pulled), then `GET /api/ps` (the models loaded) when the
first lists it.

| Status | Means | A call |
| --- | --- | --- |
| `ready` | pulled and loaded | is relayed |
| `stopped`, `not-loaded` | pulled, not loaded | is relayed, and Ollama loads the model for it |
| `unreachable`, `model-not-pulled` | the server does not list it | is refused `503 model-not-pulled` |
| `unreachable`, `no-answer` | the server did not answer | is relayed, and fails as any unreachable endpoint's call does |

The status list names the model's kind as `ollama`. A status read ends within 5 s and is reused
for 10 s, as for any hosted model. A wake is `path-unknown`: the gateway starts and loads nothing
itself.

```json
{"error":{"code":"model-not-pulled","message":"the model is not pulled on its Ollama server, and the gateway pulls no model: local"}}
```

The refusal names the alias. Pull the model with `ollama pull` and call again.

A model deleted from the server shortly after a status read is still relayed until the read is
10 s old, and the client gets Ollama's own `404` for it.

## What the gateway sends the server

Only `GET /api/tags`, `GET /api/ps`, calls on the three wire paths, and
`POST /v1/messages/count_tokens` when the model declares `context_window` and a messages call
sets `max_tokens`, or when a client asks for a count itself. Never `/api/pull`, `/api/create`,
`/api/copy`, `/api/delete` or `/api/push`. The tokens Ollama reports in its answers reach the
usage record as a hosted model's do.
