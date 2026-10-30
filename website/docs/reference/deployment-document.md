---
title: The deployment document
sidebar_position: 2
description: Every key of the closed TOML document b10x-llm-gateway reads, with its default and its rule, and how each file it names is read.
lede: One closed TOML document configures the binary; an unknown key at any level refuses the start, naming its line and the keys allowed there.
source: "Written by hand, checked against crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/trusted.rs, crates/llm-gateway-cli/src/hosted.rs, crates/llm-gateway-cli/src/routes.rs, crates/llm-gateway-cli/tests/config.rs, crates/llm-gateway-cli/tests/binary.rs, crates/llm-gateway-cli/tests/hosted.rs and crates/llm-gateway-cli/tests/routes.rs; the specification is spec/domains/deployment.yaml. The refusals below are pasted from runs of the binary."
---

# The deployment document

`b10x-llm-gateway --config <file>` reads one TOML document. It is **closed**: an unknown key at
any level is refused as `config:schema`, naming the line and the keys allowed there; a value
outside its rule is refused as `config:value`. Either refusal exits 1 before anything is bound.

```text
b10x-llm-gateway: refused config:schema: line 16: unknown field `runpod_api_key_file`, expected one of `provider`, `wires`, `context_window`, `hf_model`, `image`, `gpu_types`, `max_model_len`, `max_num_seqs`, `gpu_util`, `profiles`, `disk_gb`, `thinking`, `reasoning_effort`, `sampling`, `extra_vllm_args`, `network_volume_id`, `volume_mount_path`, `data_center_ids`, `idle_timeout_minutes`, `start_wait_seconds`, `request_hold_seconds`, `vllm_api_key_file`, `tool_calling`, `price_per_hour_usd`
```

The document holds no Runpod API key: that key stays in the connectors keyring, which is why
`runpod_api_key_file` above is refused. The specification of the document is the domain
`llm-gateway.deployment`, in
[`spec/domains/deployment.yaml`](https://github.com/beyond10x/llm-gateway/blob/main/spec/domains/deployment.yaml).

## Top level

| Key | Required | Rule |
| --- | --- | --- |
| `listen` | yes | A socket address, `127.0.0.1:8080`. An address that cannot be bound is refused as `listen:bind`. |
| `ingress_auth` | no | `bearer`, the default: every model call takes the owner secret. `none`: model calls take no credential, and the binary says so on standard error before it listens ([model calls without a credential](../concepts/single-owner-gateway.md#model-calls-without-a-credential)). Any other value is `config:schema`. |
| `owner_secret_file` | with `bearer` | The file holding the owner's secret. A relative path is read from the working directory. Required unless `ingress_auth` is `none` (`config:value`); without it, `/metrics` and `/v1/routes` refuse every request. |
| `metrics_listen` | no | A top-level key beside `listen`: a socket address for a second listener that answers `GET /metrics` with the same counters and nothing else: it takes no owner secret, every other method or path is `404 path-unknown`, and no model route is reachable on it ([scrape without the owner secret](../guides/scrape-the-counters.md#scrape-without-the-owner-secret)). Absent by default, which binds none. It must differ from `listen` (`config:value`); an address that cannot be bound is refused as `listen:bind`, naming `metrics_listen`. It stops with the gateway. |
| `metrics_bearer_file` | no | The file holding the token a scrape of `metrics_listen` must send as `authorization: Bearer`; without it that listener is open, so bind it only on an address a network policy guards. Allowed only with `metrics_listen` (`config:value`). Read once at start by the [credential-file rules](#credential-files), refused as `metrics-bearer:<rule>`; a token under 32 bytes, the owner secret's floor, is refused as `metrics-bearer:not-a-token`. |
| `[providers.<name>]` | no | Providers models name. A name is 1 to 64 of `a-z`, `0-9`, `-`, `_`, `.`, without a leading dash. |
| `routes` | no | The path of an llm `llm.routes/1` document whose routes are served beside the tables below ([`routes`](#routes)). A relative path is read from the working directory. |
| `[models.<alias>]` | at least one model, here, in `hosted` or in `routes` | The models Runpod pods serve, under the same naming rule. |
| `[hosted.<alias>]` | at least one model, here, in `models` or in `routes` | The models hosted endpoints serve, under the same naming rule; no alias may be in both tables. |

## `[providers.<name>]`

| Key | Default | Rule |
| --- | --- | --- |
| `kind` | required | `runpod-vllm` |
| `cloud_type` | `SECURE` | `SECURE` or `COMMUNITY` |
| `[providers.<name>.connectors]` | absent | The `connectors` connection for the Runpod transport. At most one provider declares one. |
| `daily_cap_usd` | absent | Above 0, at most 1000000: the most this provider's account may spend in one UTC day. A pod start of one of this provider's models that would pass it is refused `spend-ceiling`; a ceiling binds no other provider's pods. |
| `total_cap_usd` | absent | Above 0, at most 1000000: the most this provider's account may spend in all. At most one provider declares a spend ceiling, and then every model declares `price_per_hour_usd`. |

:::caution[A connectors provider starts real, billed pods]
With a `connectors` table and real Runpod credentials, the binary creates pods on that account
and reaches each one at its `https` proxy endpoint, with the certificate verified. The keys are
`executable` (an absolute path), `adapter`, `connection` (1 to 128 printable ASCII characters, no
space or leading dash), `work_directory` (an absolute path) and `timeout_seconds` (1 to 600,
default 60).
:::

A spend ceiling is checked before every pod create against the account's reported spend, the
running pods' estimates and the new pod's estimate over its startup deadline. The reported spend
is read through a connectors operation that no connectors release offers yet, so until one does,
a provider that declares a ceiling starts no pod and every start is refused `spend-ceiling`.

## `[models.<alias>]`

| Key | Default | Rule |
| --- | --- | --- |
| `provider` | required | A declared provider. |
| `wires` | required | A non-empty set of `chat`, `responses`, `messages`, none repeated. |
| `tool_calling` | `absent` | `parsed` or `absent`. A non-empty `tools` array sent to an `absent` model is `tools-not-served`. |
| `context_window` | required | 4096 to 2000000 tokens. |
| `max_model_len` | required without `profiles` | 1024 to `context_window`. |
| `hf_model` | required | 1 to 200 printable ASCII characters. |
| `image` | required | 1 to 200 printable ASCII characters. |
| `gpu_types` | required without `profiles` | 1 to 16 candidates, tried in order, each 1 to 64 printable ASCII characters. |
| `profiles` | absent | 1 to 8 `[[models.<alias>.profiles]]` tables, tried in order (below). Refused beside `gpu_types`, `max_model_len`, `max_num_seqs`, `gpu_util` or `extra_vllm_args`. |
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
| `price_per_hour_usd` | absent | Above 0, at most 1000: the most the model's pod costs per hour on any of its GPU types, which the spend check prices it by. Required once a provider declares a spend ceiling. |

Without `profiles`, `gpu_types`, `max_model_len`, `max_num_seqs`, `gpu_util` and
`extra_vllm_args` are the model's one profile, named `default`. With it, each
`[[models.<alias>.profiles]]` table declares them for one profile: `name` (required, 1 to 32 of
`A-Za-z0-9-_`), `gpu_types` and `max_model_len` (required), `max_num_seqs`, `gpu_util` and
`extra_vllm_args`, under the rules above; any other key is `config:schema`. A start tries the
first profile's GPU types in order, and moves on to the next profile only when Runpod refused
every one of them for capacity, inside the same startup deadline and with the spend check before
each create; once `start_wait_seconds` have passed since the start began, no further create is
sent and the start ends `no-capacity`. A request is clamped to the `max_model_len` of the profile
its pod was started on, and `GET /v1/models` lists the smallest among the profiles. Two profiles sharing a name or a GPU type are refused as `config:value` with the rule
`profile-conflict`. Every other key of the model, `thinking` and `sampling` included, holds for
every profile. The usage record names the profile of the pod that served a request (`profile`).

The Runpod settings reach a pod only once the binary can start one; today they are checked and
held. Two proven model declarations are in
[`docs/model-profiles.md`](https://github.com/beyond10x/llm-gateway/blob/main/docs/model-profiles.md).

## `[hosted.<alias>]` {#hosted-models}

A model served by a hosted endpoint: any server that speaks the model's wires, a vendor API or an
OpenAI-compatible server somebody else runs. Nothing is started or stopped for it. A request goes
to the endpoint's base path joined to the wire path (below), and the route inventory lists each
of its targets with its `auth_kind` and `billing_kind`, never its URL or credential.

| Key | Default | Rule |
| --- | --- | --- |
| `wires` | required | A non-empty set of `chat`, `responses`, `messages`, none repeated. |
| `base_url` | required | `http://` or `https://`, a `host[:port]` of at most 256 characters, then an optional base path: no query, fragment or user information. A base path is 1 to 32 segments of `A-Z`, `a-z`, `0-9`, `-`, `.`, `_`, `~`, at most 256 bytes, with no empty, `.` or `..` segment; one final `/` is dropped. An `http` URL for a `bearer` or `api-key` endpoint must name a loopback host (`127.0.0.0/8`, `::1` or `localhost`): a credential is never sent in cleartext to a remote host. |
| `auth_kind` | required | `anonymous`, `bearer` or `api-key`. |
| `credential_file` | absent | The file holding the credential: required unless `auth_kind` is `anonymous`, and refused when it is. |
| `credential_header` | absent | The header an `api-key` endpoint receives the credential in, 1 to 64 token characters: required for `api-key`, refused otherwise, and never `authorization`, `host` or a header that frames a message. |
| `[hosted.<alias>.headers]` | absent | Static, non-secret request headers, name to string value, sent with every request (below). On an OpenCode Go endpoint a static `x-opencode-session` is only a fallback. |
| `billing_kind` | `metered` | `metered`, `subscription` or `self-hosted`. |
| `upstream_model` | the alias | The model name the endpoint expects, 1 to 256 printable ASCII characters. |
| `context_window` | absent | 4096 to 2000000 tokens, what the public routes publish. |
| `tool_calling` | `absent` | `parsed` or `absent`. |
| `reasoning_effort` | absent | `low`, `medium` or `xhigh`: the effort a Messages request that names none is sent with. Any other value, `high` and `max` included, is `config:value`. |

Every wire path starts `/v1/`. A `base_url` without a path, or with only `/`, is asked on the
wire path itself. A base path replaces the wire path's leading `/v1`, so a `base_url` written
the way OpenAI-compatible clients write one, ending in `/v1`, works as written:

| `base_url` | Chat request goes to |
| --- | --- |
| `https://api.example.test` | `/v1/chat/completions` |
| `https://api.example.test/v1` | `/v1/chat/completions` |
| `https://openrouter.ai/api/v1` | `/api/v1/chat/completions` |
| `https://gateway.example.test/openai` | `/openai/chat/completions` |

The output clamp's count on the Messages wire follows the same rule
(`/api/v1/messages/count_tokens` for the third row). The `host` header is the authority alone.

A `bearer` endpoint is sent `authorization: Bearer <credential>`, an `api-key` endpoint
`<credential_header>: <credential>` and no `authorization`, and an `anonymous` one neither. A
credential travels over `https`, or over plain `http` only to the same machine: a `bearer` or
`api-key` endpoint at an `http` base URL whose host is not `127.0.0.0/8`, `::1` or `localhost`
is refused at start as `config:value`, naming the endpoint and quoting no credential. An
`https` endpoint is reached over the same TLS client as a pod, verified against the same roots
compiled into the binary; a certificate that does not verify is `upstream-failed` before any byte
of the request is sent. A failed endpoint is kept: the next request asks it again.

No header of the client's request reaches an endpoint, with one exception below. A vendor API
that needs a header of its own gets it from `[hosted.<alias>.headers]`: Anthropic's
`anthropic-version`, OpenRouter's `HTTP-Referer` and `X-Title`. Each is sent lowercase, right
after `host`, in name order, on every request to that endpoint.

An endpoint whose `base_url` starts with `https://opencode.ai/inference/go/` is sent one
`x-opencode-session` on every request: the client's own `x-opencode-session` when it is one
printable ASCII token of at most 256 bytes; else `llmgw-` and 32 hex characters derived from
the client's address with a key the process draws at start; else, with no client address, the
static `x-opencode-session` declared in `[hosted.<alias>.headers]`. A malformed client value is
ignored and never sent. On a translated request, which carries no static headers, the last
step is the session llm's client mints instead. Any other endpoint gets only its declared
headers. At most 32; a name is 1 to 64 token characters, and two names
that differ only in case are refused; a value is 1 to 1024 printable ASCII characters with no
leading or trailing space, so no line break. A name the gateway writes itself or that frames a
message (`host`, `authorization`, `content-type`, `content-length`, `connection`,
`transfer-encoding`, `te`, `trailer`, `upgrade`, `expect`, `keep-alive`,
`proxy-authorization`, `proxy-connection`) is refused, and so is the endpoint's own
`credential_header`: a credential belongs in `credential_file`. Every refusal is `config:value`
naming `hosted.<alias>.headers`, and quotes no value.

```toml
[hosted.claude]
wires = ["messages"]
base_url = "https://api.anthropic.com"
auth_kind = "api-key"
credential_file = "/etc/llm-gateway/anthropic-key"
credential_header = "x-api-key"
upstream_model = "claude-model"

[hosted.claude.headers]
anthropic-version = "2023-06-01"
```

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

### `kind = "bedrock"` {#bedrock-models}

`kind` is `endpoint` by default. A table with `kind = "bedrock"` declares a model Amazon Bedrock
serves instead: it takes `region`, `model_id` and `credential_source = "container"`, and
optionally `api`, `endpoint`, `billing_kind`, `context_window`, `max_output_tokens` (1 to the
window, which it is when undeclared; it bounds the output a turn naming none is given),
`anthropic_beta` (default empty: the `anthropic-beta` names a Messages client's header may carry
into an `anthropic-invoke` model's InvokeModel body, at most 32 distinct names by llm's beta name
rule; refused on a `converse` model, an empty list too) and `tool_calling` (default `parsed`). Every request to it is translated and signed with the container credentials named by
`AWS_CONTAINER_CREDENTIALS_FULL_URI` and `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`. An
endpoint's keys are refused on it, and its keys on an endpoint, each as `config:value` naming the
key. [Serve a Bedrock model](../guides/serve-a-bedrock-model.md) has every rule.

```toml
[hosted.haiku]
kind = "bedrock"
region = "eu-central-1"
model_id = "eu.anthropic.claude-haiku-5-5"
credential_source = "container"
```

The model id above is an example and is unverified.

## `routes` {#routes}

`routes = "<path>"` names a routes document in llm's `llm.routes/1` format: endpoints, each with
a `base_url`, a `protocol` and an `auth`, and routes, each a name for `<endpoint>/<model-id>`,
optionally with its `capabilities`. The binary reads it once at start and parses it with llm
0.19.0's own parser, which reads no secret and no other file. Each route is then served as a model
under its route name, from a hosted endpoint built from its endpoint: on the endpoint's wire
alone, relayed byte for byte, and translated from the other two
([translate between wires](../guides/translate-between-wires.md#serve-the-models-of-a-routes-document)).
A changed routes document takes a restart.

| Rule | Refusal |
| --- | --- |
| The file is read through the trusted-file reader (below): at most 2 MiB, llm's own bound, and not group- or world-writable. | `config:<rule>`, its message starting `routes <path>:` |
| A document llm refuses: malformed YAML, an unknown field, a repeated key, an unknown format, protocol, auth kind or secret source, a literal secret, a relative `file:` path, a route to an undeclared endpoint. | `config:value`, its message `routes <llm's message>`, which names a field, endpoint or route and never a value |
| A Bedrock endpoint (`bedrock: {region: ..}`), a route's `aliases` and a route's `price`, which llm admits and the gateway does not serve, are refused, never ignored: a Bedrock model is a [`kind = "bedrock"`](#bedrock-models) hosted model, and the gateway prices no routed call. | `config:value` naming `routes.<endpoint>`, `routes.<route>.aliases` or `routes.<route>.price` |
| A credentialed endpoint's `auth.secret` is a `file:` reference: an absolute path, or `~/<path>` resolved against `HOME`. An `env:` or `keychain:` secret is refused: the gateway reads credentials only from trusted files. | `config:value` naming `routes.<endpoint>.auth.secret` |
| The key file is read once at start, as a hosted model's `credential_file` is. | `endpoint-credential:<rule>`, its message starting `routes.<endpoint>.auth.secret <path>:` |
| `base_url` names the endpoint's API path, such as `https://api.anthropic.com/v1`: llm appends the wire path to it, and the gateway replaces the wire path's leading `/v1` with it, and the two agree only when the URL has a path. A URL without a path, a path outside the base-path rule of [`[hosted.<alias>]`](#hosted-models), or a credential over `http` to a host that is not loopback is refused. | `config:value` naming `routes.<endpoint>.base_url` |
| An `api-key` endpoint's `auth.header` is one a hosted model's `credential_header` may be, and on a `messages` endpoint it is not `anthropic-version` (in any case), the header every request to such an endpoint is sent. | `config:value` naming `routes.<endpoint>.auth.header` |
| A route name is not also a `[models.<alias>]` or `[hosted.<alias>]` alias. | `config:value` naming `routes.<route>` |
| A route name is at most 246 bytes, though llm admits 256: its inventory target id `<route>.<wire>` must fit the gateway's 256-byte label, and `responses` is the longest wire. | `config:value` naming `routes.` and the name's first 32 bytes, and the bound |

An endpoint whose key is `secret: "env:ANTHROPIC_API_KEY"`:

```text
b10x-llm-gateway: refused config:value: routes.anthropic.auth.secret must be a file: reference: the gateway reads a credential only from a trusted file, never from an env: variable or a keychain: entry
```

A route is served under its endpoint's credential (`bearer` or `api-key` in its `auth.header`,
or none). A route on a `messages` endpoint is sent llm's `anthropic-version: 2023-06-01` on every
request, the output clamp's count included, because Anthropic's API requires it; a route is sent
no other static header. It is relayed under its model id, its tool calling `parsed`
exactly when its capabilities say `tools`, and it lists its capabilities' `context_window`,
which is also its window for the Messages output clamp ([`[hosted.<alias>]`](#hosted-models)
has the same rule); llm's defaults for a plain route are a 200000-token window and 16384
output tokens. The route inventory lists one route per route name: one target
`<route>.<wire>`, provider and account `routes`, the endpoint's name, the route's model id,
`binding_revision` the SHA-256 of the routes file, the endpoint's `auth_kind`, and
`billing_kind` `self-hosted` for an `anonymous` endpoint, `metered` otherwise. A direct
`<endpoint>/<model-id>` name, which llm also resolves, is not a route and is answered
`model-unknown`.

## The files it reads

Every file goes through one trusted-file reader. It opens the file once, without following a
symlink in its last component, and makes every check on that one open file, so a file that grows
past its bound after the check is still refused. Each must be a regular file owned by you or root,
and UTF-8. The container token alone has its path resolved through symbolic links first, as a
Kubernetes projected volume mounts it, and every rule is checked on the file it resolves to. That
file must lie inside the directory the token path names; one outside it is refused
`container-token:symlink`.

| File | Size | Mode | Content |
| --- | --- | --- | --- |
| the document | at most 256 KiB | not group- or world-writable | TOML |
| `routes` | at most 2 MiB | not group- or world-writable | an `llm.routes/1` YAML document |
| `owner_secret_file` | 32 to 4096 bytes | no group or other permission at all | one printable token; trailing ASCII whitespace is trimmed |
| `vllm_api_key_file` | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| `credential_file` | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| a route endpoint's `file:` secret | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| `metrics_bearer_file` | 32 to 4096 bytes | no group or other permission at all | one printable token; trailing ASCII whitespace is trimmed |
| `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` (a Bedrock model only) | at most 16386 bytes | not group- or world-writable | one line of printable ASCII, at most 16384 bytes after one line end |

A broken rule is refused as `<source>:<rule>`, the source being `config`, `owner-secret`,
`vllm-api-key`, `endpoint-credential`, `metrics-bearer` or `container-token` (a routes document is a `config` file, a route's key file an `endpoint-credential` one):

```text
b10x-llm-gateway: refused owner-secret:unsafe-mode: owner-secret: has mode 644; none of 077 may be set
```

[Refusals](refusals.md#startup-refusals) lists every startup code.

### Credential files {#credential-files}

A hosted model's `credential_file`, a route endpoint's `file:` secret and `metrics_bearer_file`
are read by the same rules (`metrics_bearer_file` refused as `metrics-bearer:<rule>`). Anything that writes one, by hand,
[`b10x-llm-gateway credential set`](../guides/store-an-upstream-key.md) or another tool, aims at
this reader:

| Rule | Refused as |
| --- | --- |
| The path's last component is not a symbolic link. | `endpoint-credential:symlink` |
| It is a regular file. | `endpoint-credential:not-regular` |
| It is owned by the user the gateway runs as, or by root. | `endpoint-credential:untrusted-owner` |
| No group or world permission bit is set (mode `0600` or `0400`). | `endpoint-credential:unsafe-mode` |
| It holds at most 4098 bytes: a 4096-byte key and a CRLF. | `endpoint-credential:too-large` |
| The bytes are UTF-8. | `endpoint-credential:not-utf8` |
| Once trailing ASCII whitespace (a newline, a CRLF) is trimmed, it is one printable ASCII token: 1 to 4096 bytes of `!` to `~`, no space, tab or control byte. | `endpoint-credential:too-large` over 4096 bytes, `endpoint-credential:not-a-token` otherwise |

The gateway reads each such file once, at start: a key stored while it serves is used from its
next start. `credential set` writes exactly the key's bytes, with no newline, mode `0600`, and
reads the result back through this reader before it replaces the old file. It also requires the
file's directory to exist and to be owned by you or root and not group- or world-writable, so that
no other user can replace the file.
