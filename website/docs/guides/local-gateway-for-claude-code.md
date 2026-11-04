---
title: A local gateway for Claude Code
sidebar_position: 2
description: Run the gateway on loopback in front of Claude Code, with the Claude subscription passed through as-is and a self-hosted model as the sub-agent route, then start Claude Code with b10x-llm-gateway claude.
lede: One gateway on 127.0.0.1 serves Claude Code both ways. Claude models reach Anthropic on your own login, untouched; a sub-agent route reaches a model you host. The dashboard shows every call, its tokens, its cache and the subscription's limits.
source: crates/llm-gateway-cli/src/launch.rs, crates/llm-gateway-cli/src/config.rs, crates/llm-gateway/src/relay/passthrough.rs, crates/llm-gateway-cli/tests/launch_claude.rs, crates/llm-gateway-cli/tests/passthrough_encoded.rs
---

# A local gateway for Claude Code

The gateway listens on loopback and authenticates nobody. Claude Code keeps its own login: the
gateway forwards its `authorization` line to Anthropic with the request as Claude Code wrote it,
and never stores, logs or shows the value. A second route reaches a model you host, and the
launcher points Claude Code's sub-agents at it.

:::caution[Not a shared gateway]
A route that forwards the client's own credential is admitted only behind `ingress_auth = "none"`,
and such a gateway serves model calls only on a loopback address. It is for one person on one
machine.
:::

## The document

`~/.config/llm-gateway/config.toml` is the default document of `serve`, `claude` and `codex`:

```toml
listen = "127.0.0.1:8091"
ingress_auth = "none"

[hosted.claude]
wires = ["messages"]
base_url = "https://api.anthropic.com"
auth_kind = "client"
billing_kind = "subscription"
tool_calling = "parsed"
model_prefix = "claude-"

[hosted.qwen]
wires = ["chat", "responses", "messages"]
base_url = "https://<pod>-8000.proxy.runpod.net/v1"
upstream_model = "<served model name>"
auth_kind = "bearer"
credential = "keychain:llm-gateway/pod-key"
context_window = 262144
compact_window = 120000
tool_calling = "parsed"

[providers.runpod]
kind = "runpod-vllm"
api_key = "keychain:llm-gateway/runpod-api-key"

[launch.claude]
main_model = "opus"
implementer_route = "qwen"
```

- `[hosted.claude]` serves every model name starting `claude-`, relayed as sent
  ([the client's own credential](../reference/deployment-document.md#client-routes)). A 429, 502,
  503 or 529 is retried only for a client that does not retry on its own; Claude Code sends
  `x-stainless-retry-count`, so its calls get one attempt each and Claude Code retries them.
- `[hosted.qwen]` is a vLLM server on a Runpod pod you started yourself. Its key is a keychain
  entry ([keychain references](../reference/deployment-document.md#keychain-references)), never a
  value in the file. Because its address is a Runpod proxy address, the dashboard reads the
  server's own `/metrics` for its prefix cache, queue and KV cache, and shows the pod read-only,
  marked external: the gateway never stops or changes it.
- `[providers.runpod]` with a keychain `api_key` adds the account's balance and spend rate to the
  dashboard. Leave it out and the gateway reads no account.

Start it:

```bash
b10x-llm-gateway serve
```

```text
b10x-llm-gateway: warning ingress auth is off (ingress_auth = "none"): model calls are served without a credential
b10x-llm-gateway: listening on 127.0.0.1:8091
```

## Start Claude Code

```bash
b10x-llm-gateway claude
```

It reads the running gateway's routes, writes a settings overlay (mode 0600, under
`$XDG_RUNTIME_DIR`, deleted when Claude Code exits) and runs `claude --settings <overlay>`. Every
argument after `claude` is passed on. The overlay sets `ANTHROPIC_BASE_URL` to the gateway,
`CLAUDE_CODE_SUBAGENT_MODEL` to the implementer route, and the `/model` picker and auto-compact
window of each route; it names no credential, so Claude Code keeps your login. Unless you name
one, it starts on `main_model`.

The `model` parameter of Claude Code's Agent tool can name only Claude models, so a sub-agent
reaches the implementer route only when the main model leaves that parameter out. The launcher
tells it so with `--append-system-prompt`:

```text
To run a sub-agent on `qwen` (<served model name>), call the Agent tool without the `model` parameter: CLAUDE_CODE_SUBAGENT_MODEL is `qwen` in this session. The `model` enum cannot name `qwen`. Do this whenever the user asks for a `qwen` sub-agent.
```

Your own `--append-system-prompt` text is kept; the instruction is joined after it.
`[launch.claude].subagent_instruction` replaces the text, and an empty string turns it off. A
sub-agent you start with `model: sonnet` or `model: opus` still runs on that Claude model.

The gateway does not remap Claude Code's small-model (Haiku) calls to another route: a
passthrough route relays every request as the client wrote it.

`b10x-llm-gateway codex` starts Codex the same way, on `[launch.codex].model`.

## Watch it

`http://127.0.0.1:8091/dashboard` shows the calls with every attempt and its error body, the
sessions, the routes with their tokens per second and cache, the subscription's limits as
Anthropic reports them, and the Runpod panel. `http://127.0.0.1:8091/` in a browser shows the
launcher commands. [The dashboard](https://github.com/beyond10x/llm-gateway/blob/main/docs/gateway.md#the-dashboard)
in the gateway contract defines every figure.

## Without the launcher

Point Claude Code at the gateway yourself:

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:8091 CLAUDE_CODE_SUBAGENT_MODEL=qwen claude
```

Nothing else changes: Claude Code sends its own login, and the gateway passes it through.
