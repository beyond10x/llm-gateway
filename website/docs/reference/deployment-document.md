---
title: The deployment document
sidebar_position: 2
description: Every key of the closed TOML document b10x-llm-gateway reads, with its default and its rule, and how each file it names is read.
lede: One closed TOML document configures the binary; an unknown key at any level refuses the start, naming its line and the keys allowed there.
source: "Written by hand, checked against crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/trusted.rs, crates/llm-gateway-cli/tests/config.rs and crates/llm-gateway-cli/tests/binary.rs; the specification is spec/domains/deployment.yaml. The refusals below are pasted from runs of the binary."
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
| `[models.<alias>]` | at least one | The relayed models, under the same naming rule. |

## `[providers.<name>]`

| Key | Default | Rule |
| --- | --- | --- |
| `kind` | required | `runpod-vllm` |
| `cloud_type` | `SECURE` | `SECURE` or `COMMUNITY` |
| `[providers.<name>.connectors]` | absent | The `connectors` connection for the Runpod transport. At most one provider declares one. |

:::caution[Planned: a connectors provider refuses the start]
The binary checks a `connectors` table's rules and then refuses the whole document as
`config:value`, because it cannot reach a pod's `https` endpoint yet. The keys are `executable`
(an absolute path), `adapter`, `connection` (1 to 128 printable ASCII characters, no space or leading
dash), `work_directory` (an absolute path) and `timeout_seconds` (1 to 600, default 60).
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

A broken rule is refused as `<source>:<rule>`, the source being `config`, `owner-secret` or
`vllm-api-key`:

```text
b10x-llm-gateway: refused owner-secret:unsafe-mode: owner-secret: has mode 644; none of 077 may be set
```

[Refusals](refusals.md#startup-refusals) lists every startup code.
