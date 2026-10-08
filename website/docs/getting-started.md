---
title: Getting started
sidebar_position: 2
description: Build the binary, write the smallest deployment document, start the gateway and ask it something.
lede: Everything on this page runs on loopback, starts no pod and makes no call to a provider.
source: crates/llm-gateway-cli, crates/llm-gateway/tests/public_listing.rs, crates/llm-gateway-cli/tests/binary.rs; every output below is pasted from a run of the binary
---

# Getting started

## Prerequisites

| Tool | Version | Needed for |
| --- | --- | --- |
| Rust | 1.98 or newer, edition 2024 | Building the workspace |
| `curl`, `openssl` | any | The requests and the owner secret below |

## Build the binary

The crates are not published to a registry; build from source.

```bash
git clone https://github.com/beyond10x/llm-gateway
cd llm-gateway
cargo build --release --locked -p b10x-llm-gateway-cli
gateway="$PWD/target/release/b10x-llm-gateway"
"$gateway" --version
```

```text
b10x-llm-gateway 0.5.0
```

The command line is `--config <file>`, `--help` and `--version`, and nothing else
([CLI reference](reference/cli.md)).

## Write a deployment document

In a directory of your own, write `gateway.toml`:

```toml title="gateway.toml"
listen = "127.0.0.1:8080"
owner_secret_file = "owner-secret"

[providers.runpod]
kind = "runpod-vllm"

[models.small]
provider = "runpod"
wires = ["chat", "responses", "messages"]
tool_calling = "parsed"
context_window = 65536
hf_model = "example/small-model"
image = "vllm/vllm-openai:v0.27.1"
gpu_types = ["NVIDIA L40S"]
max_model_len = 65536
```

The document is closed: an unknown key at any level refuses the start. `tool_calling = "parsed"`
says the model's pod would parse tool calls; without it the model takes no tool request and the
setup page offers no client profile for it. [The deployment document](reference/deployment-document.md)
lists every key.

Create the owner secret beside it. It must be one printable token of 32 to 4096 bytes that no
group or other user can read:

```bash
openssl rand -hex 32 > owner-secret
chmod 600 owner-secret
```

## Start it and ask it something

```bash
"$gateway" --config gateway.toml &
```

Standard error shows one line once it serves:

```text
b10x-llm-gateway: listening on 127.0.0.1:8080
```

The probes and the model listing need no credential:

```bash
curl -s http://127.0.0.1:8080/health
curl -s http://127.0.0.1:8080/ready
curl -s http://127.0.0.1:8080/v1/models
```

```text
{"status":"live"}
{"status":"ready"}
{"object":"list","data":[{"id":"small","object":"model","owned_by":"llm-gateway","max_model_len":65536,"wires":["chat","responses","messages"]}]}
```

The route inventory takes the owner credential. Without it:

```bash
curl -s http://127.0.0.1:8080/v1/routes
```

```json
{"error":{"code":"credential-absent","message":"no owner credential was presented"}}
```

With it, one route per model and one target per wire, under a `config_digest` that is the
SHA-256 of `gateway.toml`'s bytes:

```bash
curl -s -H "Authorization: Bearer $(cat owner-secret)" http://127.0.0.1:8080/v1/routes
```

```json
{"config_digest":"415baf697e9c87e1c43e3c992527c13fe404ee66bc55175dc5588915c47f7f2f","route_count":1,"routes":[{"route_id":"small","alias":"small","fallback_enabled":false,"target_count":3,"targets":[{"target_id":"small.chat","position":0,"protocol":"chat","provider":"runpod","account":"runpod","endpoint":"small","model":"small","binding_revision":"415baf697e9c87e1c43e3c992527c13fe404ee66bc55175dc5588915c47f7f2f","auth_kind":"bearer","billing_kind":"self-hosted","context_window":65536},{"target_id":"small.responses","position":1,"protocol":"responses","provider":"runpod","account":"runpod","endpoint":"small","model":"small","binding_revision":"415baf697e9c87e1c43e3c992527c13fe404ee66bc55175dc5588915c47f7f2f","auth_kind":"bearer","billing_kind":"self-hosted","context_window":65536},{"target_id":"small.messages","position":2,"protocol":"messages","provider":"runpod","account":"runpod","endpoint":"small","model":"small","binding_revision":"415baf697e9c87e1c43e3c992527c13fe404ee66bc55175dc5588915c47f7f2f","auth_kind":"bearer","billing_kind":"self-hosted","context_window":65536}]}]}
```

The digest is the same bytes `sha256sum gateway.toml` prints for this document. A model call is
admitted, and then answered `target-unavailable`, because this binary relays to no pod yet:

```bash
curl -s -H "Authorization: Bearer $(cat owner-secret)" \
  -H "content-type: application/json" \
  -d '{"model":"small","messages":[{"role":"user","content":"hi"}]}' \
  http://127.0.0.1:8080/v1/chat/completions
```

```json
{"error":{"code":"target-unavailable","message":"no model target is available"}}
```

## Stop it

```bash
kill -TERM %1
```

The gateway answers what it accepted, writes one line and exits 0. At the default log level it
also writes one `usage` event per authenticated model call, here the refused one:

```text
2026-10-08T21:19:58.427410Z  INFO usage: usage record model="small" wire=chat disposition=Refused refusal=target-unavailable status=503 response_bytes=0 duration_ms=0
b10x-llm-gateway: stopped by SIGTERM accepted=10 completed=10
```

The counts cover every request the session made, including the setup-page requests of
[Set up a client](guides/set-up-a-client.md) and the scrape of
[Scrape the counters](guides/scrape-the-counters.md), which ran against the same gateway.

## Next

- [Set up a client](guides/set-up-a-client.md): what `GET /` answers Codex, Claude Code and Loom.
- [The single-owner gateway](concepts/single-owner-gateway.md): the whole surface.
- [Refusals](reference/refusals.md): every code the gateway answers.
