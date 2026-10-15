---
title: The deployment document
sidebar_position: 2
description: Every key of the closed TOML document b10x-llm-gateway reads, with its default and its rule, and how each file it names is read.
lede: One closed TOML document configures the binary; an unknown key at any level refuses the start, naming its line and the keys allowed there.
source: "Written by hand, checked against crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/trusted.rs, crates/llm-gateway-cli/src/hosted.rs, crates/llm-gateway-cli/tests/config.rs, crates/llm-gateway-cli/tests/binary.rs and crates/llm-gateway-cli/tests/hosted.rs; the specification is spec/domains/deployment.yaml. The refusals below are pasted from runs of the binary."
---

# The deployment document

`b10x-llm-gateway --config <file>` reads one TOML document. It is **closed**: an unknown key at
any level is refused as `config:schema`, naming the line and the keys allowed there; a value
outside its rule is refused as `config:value`. Either refusal exits 1 before anything is bound.

```text
b10x-llm-gateway: refused config:schema: line 16: unknown field `runpod_api_key_file`, expected one of `provider`, `wires`, `context_window`, `hf_model`, `image`, `gpu_types`, `max_model_len`, `max_num_seqs`, `gpu_util`, `disk_gb`, `thinking`, `reasoning_effort`, `sampling`, `extra_vllm_args`, `network_volume_id`, `volume_mount_path`, `data_center_ids`, `idle_timeout_minutes`, `start_wait_seconds`, `request_hold_seconds`, `vllm_api_key_file`, `tool_calling`
```

The document holds no Runpod API key: that key stays in the connectors keyring, which is why
`runpod_api_key_file` above is refused. The specification of the document is the domain
`llm-gateway.deployment`, in
[`spec/domains/deployment.yaml`](https://github.com/beyond10x/llm-gateway/blob/main/spec/domains/deployment.yaml).

## Top level

| Key | Required | Rule |
| --- | --- | --- |
| `listen` | yes | A socket address, `127.0.0.1:8080`. An address that cannot be bound is refused as `listen:bind`. |
| `owner_secret_file` | yes | The file holding the owner's secret. A relative path is read from the working directory. |
| `[providers.<name>]` | no | Providers models name. A name is 1 to 64 of `a-z`, `0-9`, `-`, `_`, `.`, without a leading dash. |
| `[models.<alias>]` | at least one model, here or in `hosted` | The models Runpod pods serve, under the same naming rule. |
| `[hosted.<alias>]` | at least one model, here or in `models` | The models hosted endpoints serve, under the same naming rule; no alias may be in both tables. |

## `[providers.<name>]`

| Key | Default | Rule |
| --- | --- | --- |
| `kind` | required | `runpod-vllm` |
| `cloud_type` | `SECURE` | `SECURE` or `COMMUNITY` |
| `[providers.<name>.connectors]` | absent | The `connectors` connection for the Runpod transport. At most one provider declares one. |

:::caution[A connectors provider starts real, billed pods]
With a `connectors` table and real Runpod credentials, the binary creates pods on that account
and reaches each one at its `https` proxy endpoint, with the certificate verified. The keys are
`executable` (an absolute path), `adapter`, `connection` (1 to 128 printable ASCII characters, no
space or leading dash), `work_directory` (an absolute path) and `timeout_seconds` (1 to 600,
default 60).
:::

## `[models.<alias>]`

| Key | Default | Rule |
| --- | --- | --- |
| `provider` | required | A declared provider. |
| `wires` | required | A non-empty set of `chat`, `responses`, `messages`, none repeated. |
| `tool_calling` | `absent` | `parsed` or `absent`. A non-empty `tools` array sent to an `absent` model is `tools-not-served`. |
| `context_window` | required | 4096 to 2000000 tokens. |
| `max_model_len` | required | 1024 to `context_window`. |
| `hf_model` | required | 1 to 200 printable ASCII characters. |
| `image` | required | 1 to 200 printable ASCII characters. |
| `gpu_types` | required | 1 to 16 candidates, tried in order, each 1 to 64 printable ASCII characters. |
| `max_num_seqs` | 8 | 1 to 1024. |
| `gpu_util` | 0.90 | 0.1 to 1.0. |
| `disk_gb` | 80 | 10 to 2000. |
| `thinking` | `off` | `on` or `off`. |
| `reasoning_effort` | absent | `low`, `medium`, `high` or `xhigh`. |
| `sampling` | absent | A JSON object, as a string. |
| `extra_vllm_args` | `[]` | At most 64 arguments, each 1 to 256 printable ASCII characters. |
| `network_volume_id` | absent | 1 to 64 ASCII letters and digits; requires `data_center_ids`. |
| `volume_mount_path` | `/workspace` | An absolute path of at most 128 characters without a trailing slash. |
| `data_center_ids` | `[]` | At most 16, each 2 to 32 of `A-Z`, `0-9`, `-`. |
| `idle_timeout_minutes` | 30 | 0 to 1440. `0` stops a quiet pod at the first cleanup pass. |
| `start_wait_seconds` | 600 | 10 to 3600: the pod's startup deadline. |
| `request_hold_seconds` | `start_wait_seconds` | 0 to `start_wait_seconds`: how long one request waits for a starting pod. |
| `vllm_api_key_file` | absent | The file holding the key the model's vLLM server expects. |

The Runpod settings reach a pod only once the binary can start one; today they are checked and
held. Two proven model declarations are in
[`docs/model-profiles.md`](https://github.com/beyond10x/llm-gateway/blob/main/docs/model-profiles.md).

## `[hosted.<alias>]` {#hosted-models}

A model served by a hosted endpoint: any server that speaks the model's wires, a vendor API or an
OpenAI-compatible server somebody else runs. Nothing is started or stopped for it. A request goes
to the same path at the endpoint, and the route inventory lists each of its targets with its
`auth_kind` and `billing_kind`, never its URL or credential.

| Key | Default | Rule |
| --- | --- | --- |
| `wires` | required | A non-empty set of `chat`, `responses`, `messages`, none repeated. |
| `base_url` | required | `http://` or `https://` and a `host[:port]` of at most 256 characters, then at most a final `/`: no path, query, fragment or user information. An `http` URL for a `bearer` or `api-key` endpoint must name a loopback host (`127.0.0.0/8`, `::1` or `localhost`): a credential is never sent in cleartext to a remote host. |
| `auth_kind` | required | `anonymous`, `bearer` or `api-key`. |
| `credential_file` | absent | The file holding the credential: required unless `auth_kind` is `anonymous`, and refused when it is. |
| `credential_header` | absent | The header an `api-key` endpoint receives the credential in, 1 to 64 token characters: required for `api-key`, refused otherwise, and never `authorization`, `host` or a header that frames a message. |
| `billing_kind` | `metered` | `metered`, `subscription` or `self-hosted`. |
| `upstream_model` | the alias | The model name the endpoint expects, 1 to 256 printable ASCII characters. |
| `context_window` | absent | 4096 to 2000000 tokens, what the public routes publish. |
| `tool_calling` | `absent` | `parsed` or `absent`. |
| `reasoning_effort` | absent | `low`, `medium` or `xhigh`: the effort a Messages request that names none is sent with. Any other value, `high` and `max` included, is `config:value`. |

A `bearer` endpoint is sent `authorization: Bearer <credential>`, an `api-key` endpoint
`<credential_header>: <credential>` and no `authorization`, and an `anonymous` one neither. A
credential travels over `https`, or over plain `http` only to the same machine: a `bearer` or
`api-key` endpoint at an `http` base URL whose host is not `127.0.0.0/8`, `::1` or `localhost`
is refused at start as `config:value`, naming the endpoint and quoting no credential. An
`https` endpoint is reached over the same TLS client as a pod, verified against the same roots
compiled into the binary; a certificate that does not verify is `upstream-failed` before any byte
of the request is sent. A failed endpoint is kept: the next request asks it again.

The gateway does not start a hosted endpoint, so it cannot set the server's chat-template
defaults; a Qwen3.8 template asked for no effort thinks at `xhigh`, its deepest. With
`reasoning_effort` declared, a Messages request that names no effort, neither
`output_config.effort` nor `chat_template_kwargs.reasoning_effort`, reaches the endpoint with
`output_config.effort` set to the declared value: an `output_config` object the request already
has gains the key, and a request without one gains `"output_config":{"effort":"<value>"}`.
Nothing else in the body changes. A request that names an effort keeps its own, with `high` and
`max` still sent as `xhigh`, and Chat and Responses requests are relayed as sent. A
`[models.<alias>]` model's `reasoning_effort` is its pod's default already, so nothing is added
for it.

```toml
[hosted.vendor-large]
wires = ["chat", "responses"]
base_url = "https://api.example.test"
auth_kind = "api-key"
credential_file = "/etc/llm-gateway/vendor-key"
credential_header = "x-api-key"
upstream_model = "vendor-large-2"
```

## The files it reads

Every file goes through one trusted-file reader. It opens the file once, without following a
symlink in its last component, and makes every check on that one open file, so a file that grows
past its bound after the check is still refused. Each must be a regular file owned by you or root,
and UTF-8.

| File | Size | Mode | Content |
| --- | --- | --- | --- |
| the document | at most 256 KiB | not group- or world-writable | TOML |
| `owner_secret_file` | 32 to 4096 bytes | no group or other permission at all | one printable token; trailing ASCII whitespace is trimmed |
| `vllm_api_key_file` | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| `credential_file` | at most 4096 bytes | no group or other permission at all | one non-empty printable token |

A broken rule is refused as `<source>:<rule>`, the source being `config`, `owner-secret`,
`vllm-api-key` or `endpoint-credential`:

```text
b10x-llm-gateway: refused owner-secret:unsafe-mode: owner-secret: has mode 644; none of 077 may be set
```

[Refusals](refusals.md#startup-refusals) lists every startup code.
