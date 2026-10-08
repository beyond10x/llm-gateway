---
title: Set up a client
sidebar_position: 1
description: Ask a running gateway for the Codex, Claude Code or Loom settings of each model it serves.
lede: GET / answers, without a credential, the settings a client needs for each model, chosen by its User-Agent; it names the owner token's setting and never gives the token.
source: crates/llm-gateway/src/listing.rs, crates/llm-gateway/tests/public_listing.rs; every output below is pasted from a run of the binary on the getting-started document
---

# Set up a client

Start the gateway as in [Getting started](../getting-started.md). Its document declares one model,
`small`, on all three wires with `tool_calling = "parsed"`.

`GET /` picks its answer from the lowercased `User-Agent`, by the first rule that holds:

| `User-Agent` | Answer | Content type |
| --- | --- | --- |
| contains `codex` | a Codex `config.toml` profile per model | `text/plain; charset=utf-8` |
| contains `claude` or `anthropic` | Claude Code environment variables per model | `text/plain; charset=utf-8` |
| starts with `mozilla/` | an HTML page | `text/html; charset=utf-8` |
| anything else, or none | plain text with the llm catalog lines a Loom run needs | `text/plain; charset=utf-8` |

A client gets a profile for a model only when the model declares the client's wire (Codex
`responses`, Claude Code `messages`, Loom `chat`) **and** `tool_calling = "parsed"`. Each answer
counts one `llmgw_instruction_views_total`.

:::caution[Planned: no client has run a live session]
These settings point a client at this gateway. The shipped binary relays to no Runpod pod yet, so
a model call from any of these clients is answered `target-unavailable`. One recorded live session
each of Claude Code, Codex and Loom against a pod is planned.
:::

## Codex

```bash
curl -s -A codex_cli_rs/0.160.0 http://127.0.0.1:8080/
```

```toml
# Codex profiles for this llm-gateway, one per model it serves on the responses wire with
# tool calling parsed. Save one as the file it names, export B10X_GATEWAY_TOKEN as the
# gateway's owner token, and start Codex with `codex --profile <alias>`.

# ~/.codex/small.config.toml
model = "small"
model_provider = "b10x-gateway"
model_context_window = 65536

[model_providers.b10x-gateway]
name = "b10x llm-gateway"
base_url = "http://127.0.0.1:8080/v1"
env_key = "B10X_GATEWAY_TOKEN"
wire_api = "responses"
```

## Claude Code

```bash
curl -s -A claude-cli/2.1.293 http://127.0.0.1:8080/
```

```bash
# Claude Code settings for this llm-gateway, one block per model it serves on the messages
# wire with tool calling parsed. Set ANTHROPIC_AUTH_TOKEN to the gateway's owner token, or
# name an apiKeyHelper that prints it; ANTHROPIC_API_KEY is sent as x-api-key, which this
# gateway does not read.

# model small
export ANTHROPIC_BASE_URL='http://127.0.0.1:8080'
export ANTHROPIC_MODEL='small'
export ANTHROPIC_DEFAULT_OPUS_MODEL='small'
export ANTHROPIC_DEFAULT_SONNET_MODEL='small'
export ANTHROPIC_DEFAULT_HAIKU_MODEL='small'
export CLAUDE_CODE_SUBAGENT_MODEL='small'
export CLAUDE_CODE_MAX_CONTEXT_TOKENS='65536'
```

## Loom, through an llm catalog

```bash
curl -s http://127.0.0.1:8080/
```

```text
llm-gateway

GET /v1/models lists the models this gateway serves. GET / with a User-Agent naming codex
answers Codex profiles, and one naming claude answers Claude Code settings. Every model call
carries the gateway's owner token as Authorization: Bearer.

# Loom, through an llm catalog (format = "llm.catalog/1"), one serving model per model this
# gateway serves on the chat wire with tool calling parsed. Give each table its ids, and the
# account a secret_reference_id that resolves to the owner token.

# model small
[[accounts]]
auth_kind = "bearer"

[[endpoints]]
base_url = "http://127.0.0.1:8080/v1"

[[models]]
upstream_name = "small"

[[serving_models]]
protocol = "chat-completions"
[serving_models.capabilities]
tools = true
context_window = 65536
```

The catalog format is llm's ([LLM](https://beyond10x.github.io/llm/),
[GitHub](https://github.com/beyond10x/llm)); [Loom](https://beyond10x.github.io/loom/)
([GitHub](https://github.com/beyond10x/loom)) reads it through llm's client crates.

## Where the base URL comes from

The base URLs use `http://` and the request's `host` header when it is only letters, digits, `.`,
`-`, `_`, `:`, `[` and `]`, and the listener's address otherwise.

## The model listing

`GET /v1/models` lists the same models in the OpenAI list shape, also without a credential:

```bash
curl -s http://127.0.0.1:8080/v1/models
```

```json
{"object":"list","data":[{"id":"small","object":"model","owned_by":"llm-gateway","max_model_len":65536,"wires":["chat","responses","messages"]}]}
```
