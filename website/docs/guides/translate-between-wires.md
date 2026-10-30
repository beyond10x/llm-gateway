---
title: Translate between wires
sidebar_position: 4
description: When the gateway relays a request byte for byte and when it translates it to the wire the model's upstream speaks, what each wire's translated subset accepts, how Claude Code's and Codex's turns are carried, and what is still refused.
lede: A model's declared wires are the wires its upstream speaks. A request on one of them is relayed unchanged; a request on another is translated within llm's published subset, and a field outside it is refused, never dropped.
source: docs/gateway.md (Translation), crates/llm-gateway/src/translate.rs, crates/llm-gateway-cli/src/translate.rs, crates/llm-gateway-cli/src/routes.rs, crates/llm-gateway-cli/tests/translation.rs, crates/llm-gateway-cli/tests/routes.rs, contracts/gateway/scenarios/translate-*.yaml and routes-*.yaml; every output below is pasted from a run of the binary
---

# Translate between wires

The binary answers all three wires for every model. When the client's wire is one the model
declares, the request goes to the upstream byte for byte, as [the relay](../concepts/relay.md)
describes. When it is not, the gateway decodes the request with llm 0.19.0's decoder for the
client's wire, sends the turn to the upstream with llm's client for the model's **first** declared
wire, and writes the answer back on the client's wire with llm's encoder.

| Client wire | Upstream `chat` | Upstream `responses` | Upstream `messages` |
| --- | --- | --- | --- |
| `chat` | relayed byte for byte | translated | translated |
| `responses` | translated | relayed byte for byte | translated |
| `messages` | translated | translated | relayed byte for byte |

:::caution[Not run against a vendor API]
Translation is tested against loopback fixture upstreams whose answers are written from llm's own
fixtures, never against a vendor API or a pod. It is translation of a published subset, not a
claim that a vendor API is reproduced.
:::

## Try it

A hosted model whose endpoint speaks only `messages`, at a loopback port where nothing listens:

```toml title="gateway.toml"
listen = "127.0.0.1:8080"
owner_secret_file = "owner-secret"

[hosted.claude]
wires = ["messages"]
base_url = "http://127.0.0.1:9/v1"
auth_kind = "anonymous"
tool_calling = "parsed"
context_window = 32768
```

The listing names every wire the model takes, and the ones its upstream speaks:

```bash
curl -s http://127.0.0.1:8080/v1/models
```

```json
{"object":"list","data":[{"id":"claude","object":"model","owned_by":"llm-gateway","wires":["chat","responses","messages"],"upstream_wires":["messages"]}]}
```

A chat request naming `n`, and a responses request asking for structured output with
`text.format`, are refused before any target is asked for:

```bash
token="authorization: Bearer $(cat owner-secret)"
curl -s http://127.0.0.1:8080/v1/chat/completions -H "$token" \
  -d '{"model":"claude","n":2,"messages":[{"role":"user","content":"Hi"}]}'
curl -s http://127.0.0.1:8080/v1/responses -H "$token" \
  -d '{"model":"claude","text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"}}},"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],"stream":true,"store":false,"include":["reasoning.encrypted_content"]}'
```

```json
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
```

The answer names only the code. The gateway's standard error names what was refused, one `info`
line per refusal, by path or field name and never by a value the client sent. Any other key
outside the subset is a key the client chose and is written `?` after its location, such as
`text.?`:

```text
2026-10-30T06:21:22.887008Z  INFO llm_gateway_cli::translate: translation refused model="claude" ingress=chat refusal=translation-unsupported refused="chat request field `n` is outside the published subset"
2026-10-30T06:21:22.891160Z  INFO llm_gateway_cli::translate: translation refused model="claude" ingress=responses refusal=translation-unsupported refused="request body carries a field outside the pinned subset: text.format"
```

A responses request carrying Codex's `instructions`, and a chat request, are inside the subset:
each is translated and sent to the endpoint as a messages request; nothing listens there, so each
fails before any output:

```bash
curl -s http://127.0.0.1:8080/v1/responses -H "$token" \
  -d '{"model":"claude","instructions":"Be brief.","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],"stream":true,"store":false,"include":["reasoning.encrypted_content"]}'
curl -s http://127.0.0.1:8080/v1/chat/completions -H "$token" \
  -d '{"model":"claude","messages":[{"role":"user","content":"Hi"}]}'
```

```json
{"error":{"code":"upstream-failed","message":"the model target could not be reached or failed; the next request asks for a replacement"}}
{"error":{"code":"upstream-failed","message":"the model target could not be reached or failed; the next request asks for a replacement"}}
```

## Serve the models of a routes document

The deployment document may name an llm `llm.routes/1` document with its top-level `routes` key
([the rules](../reference/deployment-document.md#routes)). Each route is served as a model under
its route name, on its endpoint's wire, with the endpoint's base URL, authentication and
credential. A request on the endpoint's wire is relayed byte for byte to the endpoint's base
path; a request on another wire is translated to the endpoint's wire, under the route's
capabilities. `[models.*]` and `[hosted.*]` models keep working beside the routes.

This deployment routes Codex, which speaks Responses, to Anthropic's Messages API, and Claude
Code, which speaks Messages, to OpenRouter's Chat Completions API. The model ids are examples;
use the ones your accounts offer.

```toml title="gateway.toml"
listen = "127.0.0.1:8080"
owner_secret_file = "owner-secret"
routes = "routes.yaml"
```

```yaml title="routes.yaml"
format: llm.routes/1
endpoints:
  anthropic:
    base_url: https://api.anthropic.com/v1
    protocol: messages
    auth: {kind: api-key, header: x-api-key, secret: "file:~/.config/llm-gateway/anthropic-key"}
  openrouter:
    base_url: https://openrouter.ai/api/v1
    protocol: chat-completions
    auth: {kind: bearer, secret: "file:~/.config/llm-gateway/openrouter-key"}
routes:
  codex: anthropic/claude-sonnet-4-5
  claude: openrouter/anthropic/claude-sonnet-4.5
```

Each key file holds one token and has no group or other permission (`chmod 600`); `~/` is
resolved against `HOME`. The routes file and both key files are read once, at start:

```bash
b10x-llm-gateway --config gateway.toml
```

```text
b10x-llm-gateway: listening on 127.0.0.1:8080
```

| Client wire | Route | Endpoint wire | What the gateway does |
| --- | --- | --- | --- |
| `responses` (Codex) | `codex` | `messages` | translates, and sends the turn to `https://api.anthropic.com/v1/messages` with `x-api-key` and llm's `anthropic-version: 2023-06-01` |
| `messages` (Claude Code) | `codex` | `messages` | relays byte for byte to `https://api.anthropic.com/v1/messages` with `x-api-key` and `anthropic-version: 2023-06-01` |
| `messages` (Claude Code) | `claude` | `chat` | translates, and sends the turn to `https://openrouter.ai/api/v1/chat/completions` with `authorization: Bearer` |

A route on a Messages endpoint is sent llm's `anthropic-version` on every request, the output
clamp's `count_tokens` included, since Anthropic's API requires it; a route is sent no other
static header.

The listing names both routes on every wire, and the wire each endpoint speaks:

```bash
curl -s http://127.0.0.1:8080/v1/models
```

```json
{"object":"list","data":[{"id":"claude","object":"model","owned_by":"llm-gateway","wires":["chat","responses","messages"],"upstream_wires":["chat"]},{"id":"codex","object":"model","owned_by":"llm-gateway","wires":["chat","responses","messages"],"upstream_wires":["messages"]}]}
```

The owner's route inventory names each route's endpoint, wire, model id, kinds and limits, and
never a URL, a key file or a key. `binding_revision` is the SHA-256 of the routes file:

```bash
token="authorization: Bearer $(cat owner-secret)"
curl -s http://127.0.0.1:8080/v1/routes -H "$token"
```

```json
{"config_digest":"767d483018c9701a537f76276b5407521ac1236edb6ed1592842e77a2f56b07d","route_count":2,"routes":[{"route_id":"claude","alias":"claude","fallback_enabled":false,"target_count":1,"targets":[{"target_id":"claude.chat","position":0,"protocol":"chat","provider":"routes","account":"routes","endpoint":"openrouter","model":"anthropic/claude-sonnet-4.5","binding_revision":"4444fa6ee9ef5335002f2a7adb63d7457b2186aa0ed6ba4075865e2b86cb34f4","auth_kind":"bearer","billing_kind":"metered","context_window":200000,"max_output_tokens":16384}]},{"route_id":"codex","alias":"codex","fallback_enabled":false,"target_count":1,"targets":[{"target_id":"codex.messages","position":0,"protocol":"messages","provider":"routes","account":"routes","endpoint":"anthropic","model":"claude-sonnet-4-5","binding_revision":"4444fa6ee9ef5335002f2a7adb63d7457b2186aa0ed6ba4075865e2b86cb34f4","auth_kind":"api-key","billing_kind":"metered","context_window":200000,"max_output_tokens":16384}]}]}
```

`config_digest` is the SHA-256 of `gateway.toml` exactly as read, its final newline included.

`GET /` hands each client the settings for the models its requests reach: on the model's own
wire, or on another when the gateway translates the client's recorded first turn there without a
refusal. The gateway tries each client's turn once, at start. For this document Claude Code is
handed the `codex` route, which reaches Anthropic natively, and not `claude`: its first turn
asks for effort `max`, which reaches Chat Completions as `xhigh`, and for 32000 output tokens,
while a plain route's efforts stop at `high` and its output at 16384 tokens. Codex is handed
both routes, since its first turn, at effort `high`, translates to either wire:

```bash
curl -s http://127.0.0.1:8080/ -A "claude-cli/2.1.0 (external, cli)"
curl -s http://127.0.0.1:8080/ -A "codex_cli_rs/0.50.0"
```

```text
# Claude Code settings for this llm-gateway, one block per model it serves on the messages
# wire with tool calling parsed. Set ANTHROPIC_AUTH_TOKEN to the gateway's owner token, or
# name an apiKeyHelper that prints it; ANTHROPIC_API_KEY is sent as x-api-key, which this
# gateway does not read.

# model codex
export ANTHROPIC_BASE_URL='http://127.0.0.1:8080'
export ANTHROPIC_MODEL='codex'
export ANTHROPIC_DEFAULT_OPUS_MODEL='codex'
export ANTHROPIC_DEFAULT_SONNET_MODEL='codex'
export ANTHROPIC_DEFAULT_HAIKU_MODEL='codex'
export CLAUDE_CODE_SUBAGENT_MODEL='codex'
export CLAUDE_CODE_MAX_CONTEXT_TOKENS='200000'

# Codex profiles for this llm-gateway, one per model it serves on the responses wire with
# tool calling parsed. Save one as the file it names, export B10X_GATEWAY_TOKEN as the
# gateway's owner token, and start Codex with `codex --profile <alias>`.

# ~/.codex/claude.config.toml
model = "claude"
model_provider = "b10x-gateway"
model_context_window = 200000

[model_providers.b10x-gateway]
name = "b10x llm-gateway"
base_url = "http://127.0.0.1:8080/v1"
env_key = "B10X_GATEWAY_TOKEN"
wire_api = "responses"

# ~/.codex/codex.config.toml
model = "codex"
model_provider = "b10x-gateway"
model_context_window = 200000

[model_providers.b10x-gateway]
name = "b10x llm-gateway"
base_url = "http://127.0.0.1:8080/v1"
env_key = "B10X_GATEWAY_TOKEN"
wire_api = "responses"
```

Claude Code on `codex` is relayed to Anthropic as it sends its requests, `thinking` and all,
with the route's `anthropic-version`. Codex's first turn to either route is translated and sent
([how](#claude-code-and-codex)). Claude Code's first turn reaches `claude` once the route is a
mapping whose `capabilities` name `xhigh` among its `reasoning_efforts` and allow at least
32000 `max_output_tokens`; `GET /` then profiles it there too. What llm still refuses is
refused before any request leaves the gateway: here Codex asking for structured
output with `text.format` to `codex`, Claude Code's `thinking.display` and a thinking budget,
which Chat Completions cannot hold, to `claude`:

```bash
curl -s http://127.0.0.1:8080/v1/responses -H "$token" \
  -d '{"model":"codex","instructions":"Be brief.","text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object"}}},"input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],"stream":true,"store":false,"include":["reasoning.encrypted_content"]}'
curl -s http://127.0.0.1:8080/v1/messages -H "$token" \
  -d '{"model":"claude","max_tokens":1024,"thinking":{"type":"adaptive","display":"summarized"},"messages":[{"role":"user","content":"Hi"}]}'
curl -s http://127.0.0.1:8080/v1/messages -H "$token" \
  -d '{"model":"claude","max_tokens":1024,"thinking":{"type":"enabled","budget_tokens":512},"messages":[{"role":"user","content":"Hi"}]}'
```

```json
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
```

A request inside the subset, from any client, is translated and sent.


A plain route (`<endpoint>/<model-id>`) is translated under llm's default capabilities: tools,
sampling, efforts `low`, `medium` and `high`, a 200000-token window and at most 16384 output
tokens. A route written as a mapping with its own `capabilities` is translated under those, and
its `tools` decides whether a tool request on its own wire is relayed or refused
`tools-not-served`.

## What each wire's subset accepts

Every field a decoder reads is carried, translated to the upstream wire's name for it, sent only
where the upstream wire has it (a hint, such as a cache key or verbosity), or refused by name.
Every other key is refused by its path, at every level, never dropped.

- **Chat**: `model`, `messages` (`system` or `developer` first, `user`, `assistant` with text and
  `tool_calls`, `tool`), `tools` (`function`, not `strict`), `tool_choice` (`auto`, `required`,
  a named function), `max_completion_tokens` or `max_tokens`, `temperature`, `top_p`,
  `reasoning_effort`, `parallel_tool_calls`, `prompt_cache_key`, `verbosity`, `stream` and
  `stream_options.include_usage`.
- **Responses**: `model`, `instructions`, `input` (leading `developer` messages, `user` and
  `assistant` messages of one text part, `function_call`, `function_call_output`, each item
  with an optional `id`), `tools` (`function` with `strict: false`), `tool_choice`,
  `max_output_tokens`, `temperature`, `top_p`, `reasoning.effort` and `reasoning.summary`,
  `parallel_tool_calls`, `prompt_cache_key`, `text.verbosity`, `client_metadata`, and exactly
  `stream: true`, `store: false` and `include: ["reasoning.encrypted_content"]`.
- **Messages**: `model`, `max_tokens`, `system` (a string or text blocks), `messages` (`text`,
  `tool_use` and `tool_result` blocks), `tools`, `stream`, `temperature` (at most 1), `top_p`,
  `thinking`, `output_config.effort`, `tool_choice` (with `disable_parallel_tool_use`),
  `metadata` and `cache_control`.

A request is also checked against the model's capabilities: tools only when its tool calling is
`parsed`, the efforts `minimal` to `xhigh` (and `none` when the upstream speaks chat or
responses) as the upstream wire receives them, and an output limit no larger than the model's
window (its `max_model_len`, else its `context_window`, else 200000). Effort `max` reaches a chat
or responses upstream as `xhigh`, and disabled thinking as `none`; a messages upstream would
receive `max` and `none` as they are, and refuses both. A Messages upstream asked by a request
that names no output limit gets half the model's window (200000 when it declares none), at most
16384 tokens, so the input keeps room.

## Claude Code and Codex

- **Claude Code's first turn** (adaptive `thinking`, a `system` of two text blocks,
  `cache_control` and effort `max`) is carried to a chat or responses upstream: the system
  blocks become one system message (a developer message on responses), the effort is `xhigh`, and `thinking` and `cache_control`,
  which those wires have no place for, are not sent.
- **Codex's first turn** (`instructions`, a developer message, item ids, `parallel_tool_calls`,
  `prompt_cache_key`, `text.verbosity`, `reasoning.summary` and `client_metadata`) is carried to
  a chat or messages upstream: the instructions and the developer message become the system
  prompt, `parallel_tool_calls` and the effort are sent under the upstream wire's names, the
  cache key and verbosity where the wire has them, and the item ids and `client_metadata` never.
- **Codex's later turns** replay the `reasoning` items of its previous answer. On a translated
  route the gateway hands a Responses client the upstream's reasoning state sealed for that
  route: each `reasoning` item's `encrypted_content` is `llmgw-seal-1.` followed by the state
  and a tag under a key the gateway generates when it starts and never writes out. The next
  turn on the same route is served: a Messages upstream gets back the signed `thinking` block it
  minted, and a Chat upstream, which mints no state, gets the turn without the item. An item the
  gateway did not seal for this route since it last started is refused
  `translation-opaque-state`: one from another route or upstream wire, an altered or forged
  seal, an upstream's own encrypted reasoning, and any item sealed before a restart. After a
  restart, start a new conversation.
- **Claude Code's later turns** replay the signed `thinking` blocks of its previous answer. No
  seal is handed to a Messages client (llm's Messages encoder places a block the stream did not
  show after every streamed one), so a translated route refuses a turn replaying such a block
  `translation-opaque-state`, as it does one signed by another upstream. `GET /` profiles
  Claude Code for a translated route by its first turn only.
- **A `system` message inside Messages `messages`**, which headless Claude Code sends after
  the user prompt, is carried to every upstream wire and to Bedrock. Leading ones join the
  system prompt in order. A later one stays at its place as user text in the mark
  `<system-message>\n<text>\n</system-message>`; after a tool call it follows the tool results.
- **Still refused by name**: structured output (`output_config.format`, `text.format`,
  `response_format`), `context_management`, `thinking.display`, hosted and custom tools, a
  thinking budget to a chat or responses upstream, and a `developer` message after the first
  item of Responses `input`.

## Refusals

| Code | Status | When |
| --- | --- | --- |
| `translation-unsupported` | 400 | a field or value outside the subset, or past its size bound |
| `translation-invalid` | 400 | a value breaking a rule of the subset, such as two output limits that disagree |
| `translation-opaque-state` | 400 | the request replays reasoning state (a `thinking` block, a `reasoning` item) the gateway did not seal for this route since it last started |
| `upstream-refused` | 502 | before any output the upstream's answer status refused the translated request (`401`, `403`, `429`, another `4xx`), or its answer broke its wire (ended cleanly before its end event, called a tool not offered) |
| `upstream-failed` | 502 | before any output the upstream could not be reached, answered `408` or `5xx`, timed out, or reset or cut its connection |
| `translation-unrepresentable` | 502 | the answer cannot be written on the client's wire, and nothing of it was |
| `translation-too-large` | 502 | the answer passed a published bound (65536 events, 32 MiB, a 1 MiB line or event, 64 KiB of one tool call's arguments), and nothing of it was written |

None of the first three asks for a target, so a refused request never wakes a pod.

## The answer

A streamed request (responses always, chat and messages with `stream: true`) is answered as the
client wire's server-sent events, event by event as the upstream answers; a non-streamed one as
its JSON object. The upstream's reported token counts are carried, and an unreported one stays
unreported. Reasoning text is shown where the client's wire has a place for it (chat
`reasoning_content`, a responses reasoning summary) and withheld on messages. An answer the
client's encoder cannot carry after the stream began ends with that wire's terminal error:
`response.failed`, an `error` event, or a chat `{"error":…}` chunk without `[DONE]`; so does one
past one of those bounds, with `translation-too-large`'s message. A client that
stops reading, or that closes while it waits for a non-streamed answer, cancels the upstream
turn; nothing is retried.

What a translated request does not get: the gateway's static `headers` of a hosted endpoint, the
Messages effort mapping, and the Messages output clamp's token count.

## Known limits

- **Answers llm's encoders refuse**: two assistant messages back to back, or a message another
  item interrupts, to a responses client; redacted thinking between streamed blocks, or tool
  arguments that are not a JSON object, to a messages client; a terminal reason a chat client has
  no name for. Before any output each is the 502 refusal `translation-unrepresentable`.
