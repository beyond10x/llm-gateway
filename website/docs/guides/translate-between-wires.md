---
title: Translate between wires
sidebar_position: 4
description: When the gateway relays a request byte for byte and when it translates it to the wire the model's upstream speaks, what each wire's translated subset accepts, and the limits that matter for Claude Code and Codex.
lede: A model's declared wires are the wires its upstream speaks. A request on one of them is relayed unchanged; a request on another is translated within llm's published subset, and a field outside it is refused, never dropped.
source: docs/gateway.md (Translation), crates/llm-gateway/src/translate.rs, crates/llm-gateway-cli/src/translate.rs, crates/llm-gateway-cli/tests/translation.rs, contracts/gateway/scenarios/translate-*.yaml; every output below is pasted from a run of the binary
---

# Translate between wires

The binary answers all three wires for every model. When the client's wire is one the model
declares, the request goes to the upstream byte for byte, as [the relay](../concepts/relay.md)
describes. When it is not, the gateway decodes the request with llm 0.6.0's decoder for the
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

A chat request naming `n`, and a responses request naming Codex's `instructions`, are refused
before any target is asked for:

```bash
token="authorization: Bearer $(cat owner-secret)"
curl -s http://127.0.0.1:8080/v1/chat/completions -H "$token" \
  -d '{"model":"claude","n":2,"messages":[{"role":"user","content":"Hi"}]}'
curl -s http://127.0.0.1:8080/v1/responses -H "$token" \
  -d '{"model":"claude","instructions":"Be brief.","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hi"}]}],"stream":true,"store":false,"include":["reasoning.encrypted_content"]}'
```

```json
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
{"error":{"code":"translation-unsupported","message":"the request uses a field or value outside the subset the gateway translates"}}
```

A chat request inside the subset is translated and sent to the endpoint as a messages request;
nothing listens there, so it fails before any output:

```bash
curl -s http://127.0.0.1:8080/v1/chat/completions -H "$token" \
  -d '{"model":"claude","messages":[{"role":"user","content":"Hi"}]}'
```

```json
{"error":{"code":"upstream-failed","message":"the model target could not be reached or failed; the next request asks for a replacement"}}
```

## What each wire's subset accepts

- **Chat**: `model`, `messages` (`system` or `developer` first, `user`, `assistant` with text and
  `tool_calls`, `tool`), `tools` (`function`), `tool_choice` (`auto`, `required`, a named
  function), `max_completion_tokens` or `max_tokens`, `temperature`, `top_p`,
  `reasoning_effort`, `stream` and `stream_options.include_usage`.
- **Responses**: `model`, `input` (one leading `developer` message, `user` and `assistant`
  messages of one text part, `function_call`, `function_call_output`), `tools` (`function` with
  `strict: false`), `tool_choice`, `max_output_tokens`, `temperature`, `top_p`,
  `reasoning.effort`, and exactly `stream: true`, `store: false` and
  `include: ["reasoning.encrypted_content"]`.
- **Messages**: `model`, `max_tokens`, `system` (a string or one text block), `messages`
  (`text`, `tool_use` and `tool_result` blocks), `tools`, `stream`, `temperature` (at most 1),
  `top_p`, `output_config.effort` and `tool_choice`.

A request is also checked against the model's capabilities: tools only when its tool calling is
`parsed`, the efforts `minimal` to `xhigh`, and an output limit no larger than the model's window
(its `max_model_len`, else its `context_window`, else 200000). Every nested level is closed
too: a key a level does not list, such as a tool's `strict`, a message's `name` or a part's
`cache_control`, is refused rather than dropped. A Messages upstream asked by a request that
names no output limit gets half the model's window (200000 when it declares none), at most
16384 tokens, so the input keeps room.

## Refusals

| Code | Status | When |
| --- | --- | --- |
| `translation-unsupported` | 400 | a field or value outside the subset, or past its size bound |
| `translation-invalid` | 400 | a value breaking a rule of the subset, such as two output limits that disagree |
| `translation-opaque-state` | 400 | the request replays reasoning state (a `thinking` block, a `reasoning` item) another upstream minted |
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

- **Claude Code** sends `thinking`, a `system` of two blocks, `cache_control` on content and
  effort `max`, each outside the messages subset, and replays signed `thinking` blocks on every
  later turn. Point Claude Code at a model whose upstream speaks `messages`; `GET /` profiles only
  those.
- **Codex** sends `instructions`, `parallel_tool_calls`, `prompt_cache_key`, `text` and
  `reasoning.summary`, each outside the responses subset. Point Codex at a model whose upstream
  speaks `responses`; `GET /` profiles only those.
- **Answers llm's encoders refuse**: two assistant messages back to back, or a message another
  item interrupts, to a responses client; redacted thinking between streamed blocks, or tool
  arguments that are not a JSON object, to a messages client; a terminal reason a chat client has
  no name for. Before any output each is the 502 refusal `translation-unrepresentable`.
