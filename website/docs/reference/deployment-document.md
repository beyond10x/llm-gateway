---
title: The deployment document
sidebar_position: 2
description: Every key of the closed TOML document b10x-llm-gateway reads, with its default and its rule, and how each file it names is read.
lede: One closed TOML document configures the binary; an unknown key at any level refuses the start, naming its line and the keys allowed there.
source: "Written by hand, checked against crates/llm-gateway-cli/src/config.rs, crates/llm-gateway-cli/src/trusted.rs, crates/llm-gateway-cli/src/hosted.rs, crates/llm-gateway-cli/src/routes.rs, crates/llm-gateway-cli/tests/config.rs, crates/llm-gateway-cli/tests/binary.rs, crates/llm-gateway-cli/tests/hosted.rs, crates/llm-gateway-cli/tests/routes.rs, crates/llm-gateway-cli/src/records.rs and crates/llm-gateway-cli/tests/interaction_records.rs; the specification is spec/domains/deployment.yaml. The refusals below are pasted from runs of the binary."
---

# The deployment document

`b10x-llm-gateway --config <file>` reads one TOML document. It is **closed**: an unknown key at
any level is refused as `config:schema`, naming the line and the keys allowed there; a value
outside its rule is refused as `config:value`. Either refusal exits 1 before anything is bound.

```text
b10x-llm-gateway: refused config:schema: line 16: unknown field `runpod_api_key_file`, expected one of `provider`, `wires`, `context_window`, `hf_model`, `image`, `gpu_types`, `max_model_len`, `max_num_seqs`, `gpu_util`, `profiles`, `disk_gb`, `thinking`, `reasoning_effort`, `sampling`, `extra_vllm_args`, `network_volume_id`, `volume_mount_path`, `data_center_ids`, `idle_timeout_minutes`, `start_wait_seconds`, `request_hold_seconds`, `vllm_api_key_file`, `vllm_api_key`, `tool_calling`, `price_per_hour_usd`, `start_on_request`, `token_price`
```

The document holds no Runpod API key: with a `connectors` table that key stays in the connectors
keyring, and a local gateway names it by an OS keychain entry or a file of its own (a provider's `api_key` or `api_key_file`), never a value, and never as
`runpod_api_key_file` in a model, which is refused above ([keychain references](#keychain-references)). The specification of the document is the domain
`llm-gateway.deployment`, in
[`spec/domains/deployment.yaml`](https://github.com/beyond10x/llm-gateway/blob/main/spec/domains/deployment.yaml).

## Top level

| Key | Required | Rule |
| --- | --- | --- |
| `listen` | yes | A socket address, `127.0.0.1:8080`. An address that cannot be bound is refused as `listen:bind`. |
| `ingress_auth` | no | `bearer`, the default: every model call takes the owner secret. `none`: model calls take no credential, and the binary says so on standard error before it listens ([model calls without a credential](../concepts/single-owner-gateway.md#model-calls-without-a-credential)). `service-account-token`: a model call takes the owner secret or a Kubernetes service-account token the cluster's TokenReview API admits ([`[token_review]`](#token-review)). Any other value is `config:schema`. |
| `owner_secret_file` | with `bearer` | The file holding the owner's secret. A relative path is read from the working directory. Required only with `bearer` (`config:value`); without it, `/metrics` and `/v1/routes` refuse every request. |
| `[token_review]` | with `service-account-token` | How a caller's service-account token is reviewed ([below](#token-review)). Required with `ingress_auth = "service-account-token"` and refused with any other value (`config:value`). |
| `metrics_listen` | no | A top-level key beside `listen`: a socket address for a second listener that answers `GET /metrics` with the same counters and nothing else: it takes no owner secret, every other method or path is `404 path-unknown`, and no model route is reachable on it ([scrape without the owner secret](../guides/scrape-the-counters.md#scrape-without-the-owner-secret)). Absent by default, which binds none. It must differ from `listen` (`config:value`) unless both name port 0, where the system picks a port for each; the binary writes `b10x-llm-gateway: metrics listening on <address>` with the address it bound, before `listening on`. An address that cannot be bound is refused as `listen:bind`, naming `metrics_listen`. It stops with the gateway. |
| `metrics_bearer_file` | no | The file holding the token a scrape of `metrics_listen` must send as `authorization: Bearer`; without it that listener is open, so bind it only on an address a network policy guards. Allowed only with `metrics_listen` (`config:value`). Read once at start by the [credential-file rules](#credential-files), refused as `metrics-bearer:<rule>`; a token under 32 bytes, the owner secret's floor, is refused as `metrics-bearer:not-a-token`. |
| `[providers.<name>]` | no | Providers models name. A name is 1 to 64 of `a-z`, `0-9`, `-`, `_`, `.`, without a leading dash. |
| `routes` | no | The path of an llm `llm.routes/1` document whose routes are served beside the tables below ([`routes`](#routes)). A relative path is read from the working directory. |
| `[models.<alias>]` | at least one model, here, in `hosted` or in `routes` | The models Runpod pods serve, under the same naming rule. |
| `[hosted.<alias>]` | at least one model, here, in `models` or in `routes` | The models hosted endpoints serve, under the same naming rule; no alias may be in both tables. |
| `[aliases.<alias>]` | no | Second names of `[models.<alias>]` models, each served by its model's pod with thinking off ([`[aliases.<alias>]`](#aliases)), under the same naming rule; no alias may be in another table. |
| `[watchdog]` | no | The heartbeat `serve` writes and the limits `b10x-llm-gateway watchdog` enforces ([`[watchdog]`](#watchdog)). Absent: no heartbeat, and `watchdog` refuses to start. |
| `[interaction_records]` | no | The SQLite file every finished model call is appended to as session records ([`[interaction_records]`](#interaction-records)). Absent: no byte of a call is copied, no writer thread runs and no file is created. |

## `[providers.<name>]`

| Key | Default | Rule |
| --- | --- | --- |
| `kind` | required | `runpod-vllm` |
| `cloud_type` | `SECURE` | Only `SECURE`. `COMMUNITY` is refused `config:schema`: every pod is a Secure Cloud, on-demand pod, and a created pod placed anywhere else is terminated at once. |
| `[providers.<name>.connectors]` | absent | The `connectors` connection for the Runpod transport. At most one provider declares one, or an `api_key` or `api_key_file`. |
| `api_key` | absent | A [`keychain:<service>/<account>`](#keychain-references) reference to the account's Runpod API key: the binary then calls Runpod's REST API v2 (at `management_url`) itself, over TLS verified against the roots compiled into it, and reads the balance from `GET /v2/billing/balance`, falling back to Runpod's GraphQL `myself { clientBalance }` when that answers 403 or 404. Refused beside a `connectors` table, and on more than one provider. The key is read once at start and refused as `endpoint-credential:<rule>`, its message starting `providers.<name>.api_key:`. A start that cannot read one complete pod listing (every page) serves the provider's models `target-unavailable`, as for an unreachable connectors connection; the start's warning names the provider, the host and the HTTP status or the resolver's error, never the key, and each `target-unavailable` message ends `: provider <name> was not reachable at start`. |
| `api_key_file` | absent | The file holding the account's Runpod API key, in place of `api_key`: the key is read once at start under the rules of `vllm_api_key_file` (see [the files it reads](#the-files-it-reads)) and used as `api_key` is. Refused beside `api_key`, beside a `connectors` table, and on more than one provider, each as `config:value` naming `providers.<name>.api_key_file`. A file that breaks a rule refuses the start before any request reaches Runpod, as `endpoint-credential:<rule>`, its message starting `providers.<name>.api_key_file:` and naming neither the path nor the key. |
| `management_url` | `https://api.runpod.io/v2` | Where Runpod's REST API v2 is: `https://`, or `http://` to a loopback host (a test fixture); a host, no user information, query or fragment; at most 2048 bytes; a trailing `/` is dropped. Read only beside `api_key` or `api_key_file`: declared without one it is refused `config:value`, as is any value outside the rule, the message naming `providers.<name>.management_url` and quoting nothing written. A failed management call never hides the last balance read and never ends a call relayed to a running pod. |
| `proxy_url` | `https://{pod_id}-{port}.proxy.runpod.net` | Where a running pod serves inference: `https://` and a host with an optional port, at most 512 bytes, whose only placeholders are `{pod_id}` (the pod's id) and `{port}` (8000, the port vLLM serves on). The pod's endpoint is the URL filled, then `/v1/`; the default fills to the address llm's Runpod description builds. Anything else is refused `config:value` at start, naming `providers.<name>.proxy_url`. |
| `daily_cap_usd` | absent | Above 0, at most 1000000: the most this provider's account may spend in one UTC day. A pod start of one of this provider's models that would pass it is refused `spend-ceiling`; a ceiling binds no other provider's pods. |
| `total_cap_usd` | absent | Above 0, at most 1000000: the most this provider's account may spend in all, as Runpod reports its pod billing over its default window. With `api_key` that window is the 30 days up to now, so this is a ceiling on the last 30 days. At most one provider declares a spend ceiling, and then every model declares `price_per_hour_usd`. |
| `spend_ledger_file` | `$XDG_STATE_HOME/llm-gateway/spend-ledger.json`, else `$HOME/.local/state/llm-gateway/spend-ledger.json` | Where the gateway keeps its spend ledger: each pod it started, the model's `price_per_hour_usd`, and when its create was sent and its stop confirmed. Billing reports a pod's spend hours late, so each ceiling counts the larger of the reported spend and the ledger's (today's part for `daily_cap_usd`, the last 30 days' for `total_cap_usd`), a pod still running up to now, and a restarted gateway reads the pods it started before. Allowed only beside `daily_cap_usd` or `total_cap_usd` (`config:value` otherwise); with neither `XDG_STATE_HOME` nor `HOME` set and no value, the start is refused `config:value`. The file is read once at start under the rules of the [document itself](#the-files-it-reads) but at most 1 MiB, and is rewritten whole, mode 0600, after each pod start and confirmed stop; a missing file is an empty ledger. A file that breaks a rule, or is not a ledger, is never written over, and every pod start under the ceiling is refused `spend-ceiling` until it is fixed or moved away. |

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
| `image` | required | 1 to 200 printable ASCII characters, starting with `vllm/vllm-openai:` and a tag; any other image is refused `config:value` when the binary starts. |
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
| `vllm_api_key` | absent | A [`keychain:<service>/<account>`](#keychain-references) reference to that key, in place of the file: refused beside `vllm_api_key_file`, and as `vllm-api-key:<rule>` when the entry cannot be read. |
| `price_per_hour_usd` | absent | Above 0, at most 1000: the most the model's pod costs per hour on any of its GPU types, which the spend check prices it by. Required once a provider declares a spend ceiling. |
| `start_on_request` | `false` | `true` or `false`. `false`: a model call never starts the model's pod; a call while the pod is stopped or stopping is refused `model-stopped` (503) at once, and only the owner's wake starts it. `true`: a call starts the pod as a wake would and is held while it starts. `true` is refused `config:value`, naming the model and the missing cap, unless the model's provider declares `daily_cap_usd`. |
| `[models.<alias>.token_price]` | absent | What the model's tokens cost, which each call's usage record names as `cost_usd` ([token prices](#token-prices)). Unrelated to `price_per_hour_usd`. |

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
of its targets with its `auth_kind` and `billing_kind`, never its URL or credential. Its status
(`GET /v1/models/{alias}/status` and `GET /v1/status`) is a probe of the endpoint, sent only
when a status is read: `GET <base path>/models` with its credential, `ready` on a 2xx answer
(and on 401 or 403 for a `client` endpoint, probed without a credential), otherwise
`unreachable` with `no-answer`, `credential-refused` or `status-<code>`. A probe ends within
5 s and its result is reused for 10 s. Its wake stays `path-unknown`. An endpoint that answers
model calls and no model list names another probe with `status_probe = "count-tokens"` (below).

A `ready` model's entry in `GET /v1/status` adds `"reason":"serves-claude-code-clients-only"`,
and its row on the dashboard says "reachable, serves Claude Code clients only", when the
gateway's own record of its last 20 relayed model calls holds at least 2 from Claude Code (a
`user-agent` of `claude-cli/…`), every one answered 2xx, and at least 2 from other clients, every
one answered `429` with an error of type `rate_limit_error`. Every call is still relayed as the
client sent it.

| Key | Default | Rule |
| --- | --- | --- |
| `wires` | required | A non-empty set of `chat`, `responses`, `messages`, none repeated. |
| `base_url` | required | `http://` or `https://`, a `host[:port]` of at most 256 characters, then an optional base path: no query, fragment or user information. A base path is 1 to 32 segments of `A-Z`, `a-z`, `0-9`, `-`, `.`, `_`, `~`, at most 256 bytes, with no empty, `.` or `..` segment; one final `/` is dropped. An `http` URL for a `bearer` or `api-key` endpoint must name a loopback host (`127.0.0.0/8`, `::1` or `localhost`): a credential is never sent in cleartext to a remote host. |
| `auth_kind` | required | `anonymous`, `bearer`, `api-key` or `client`. A `client` endpoint is sent the client's own `authorization` and `x-api-key` lines and the request as the client wrote it ([the client's own credential](#client-routes)). |
| `credential_file` | absent | The file holding the credential: this or `credential` is required unless `auth_kind` is `anonymous` or `client`, and refused when it is. |
| `credential` | absent | A [`keychain:<service>/<account>`](#keychain-references) reference to the credential, in place of `credential_file`: refused beside it, on a Bedrock model, and as `endpoint-credential:<rule>`, its message starting `hosted.<alias>.credential:`, when the entry cannot be read. `credential set` refuses such a model: store the key with the platform's tool. |
| `credential_header` | absent | The header an `api-key` endpoint receives the credential in, 1 to 64 token characters: required for `api-key`, refused otherwise, and never `authorization`, `host` or a header that frames a message. |
| `[hosted.<alias>.headers]` | absent | Static, non-secret request headers, name to string value, sent with every request (below). On an OpenCode Go endpoint a static `x-opencode-session` is only a fallback. |
| `billing_kind` | `metered` | `metered`, `subscription` or `self-hosted`. |
| `upstream_model` | the alias | The model name the endpoint expects, 1 to 256 printable ASCII characters. |
| `context_window` | absent | 4096 to 2000000 tokens, what the public routes publish. |
| `tool_calling` | `absent` | `parsed` or `absent`. |
| `reasoning_effort` | absent | `low`, `medium` or `xhigh`: the effort a Messages request that names none is sent with. Any other value, `high` and `max` included, is `config:value`. |
| `model_prefix` | absent | 1 to 64 printable ASCII characters without a space: every model name starting with it is served by this model too, relayed as the client sent it. The longest prefix wins; an alias matched exactly wins over every prefix. |
| `newest` | absent | On a `client` endpoint only: a family of 1 to 32 lower-case letters, digits and `-`, such as `opus`. The alias is then sent as the newest model of that family the endpoint lists for the client's own credential (`GET /v1/models?limit=1000`), remembered for an hour. |
| `retry_max_attempts` | 4 | On a `client` endpoint only, 1 to 10: the most attempts a call makes on a 429, 502, 503 or 529. A client that sends `x-stainless-retry-count` retries on its own and gets one attempt. |
| `retry_max_wait` | 60 | On a `client` endpoint only, 0 to 600 seconds: the most a call waits for its retries in all. |
| `compact_window` | absent | 100000 to 1000000 tokens: the auto-compact window `b10x-llm-gateway claude` gives Claude Code for this model. |
| `status_probe` | `models` | How the model's status is read. `models`: `GET <base path>/models`. `count-tokens`: one `POST <base path>/messages/count_tokens` with the credential, the static headers, `anthropic-version: 2023-06-01` unless a static header names it, and the body `{"model":"<upstream_model>","messages":[{"role":"user","content":"ping"}]}`, which asks for no generation; a 2xx answer is `ready`, any other is read as the `models` probe's, within the same bounds. `count-tokens` needs the `messages` wire. Any other value, and `count-tokens` on a model without the `messages` wire, is `config:value`. |
| `[hosted.<alias>.token_price]` | absent | What the model's tokens cost, on every kind of hosted model ([token prices](#token-prices)). |

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

`kind` is `endpoint` by default, or `bedrock` or [`ollama`](#ollama-models). A table with
`kind = "bedrock"` declares a model Amazon Bedrock
serves instead: it takes `region`, `model_id` and `credential_source = "container"`, and
optionally `api`, `endpoint`, `billing_kind`, `context_window`, `max_output_tokens` (1 to the
window, which it is when undeclared; it bounds the output a turn naming none is given),
`anthropic_beta` (default empty: the `anthropic-beta` names a Messages client's header may carry
into an `anthropic-invoke` model's InvokeModel body, at most 32 distinct names by llm's beta name
rule; refused on a `converse` model, an empty list too), `reasoning_efforts` (default `low`,
`medium`, `high`, `xhigh` and `max`: the efforts an `anthropic-invoke` model takes as
`output_config.effort`, `[]` for none; an effort outside it is `translation-unsupported`;
refused on a `converse` model) and `tool_calling` (default `parsed`). Every request to it is translated and signed with the container credentials named by
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

### `kind = "ollama"` {#ollama-models}

A table with `kind = "ollama"` declares a model on an Ollama server. It is a hosted endpoint
with these keys and defaults; every other endpoint key keeps its rule above, and a Bedrock key is
refused as on an endpoint.

| Key | Default | Rule |
| --- | --- | --- |
| `base_url` | `http://127.0.0.1:11434` | The server's root: `http://` or `https://` and a `host[:port]`, on any host, with no path. A path, `/v1` included, is `config:value`: the gateway adds `/v1` and `/api` itself. |
| `wires` | all three | Each named wire is relayed to Ollama's own path: `/v1/chat/completions`, `/v1/responses`, `/v1/messages`. A wire left out is translated onto the first one named. |
| `upstream_model` | the alias | The Ollama model name, such as `qwen2.5:1.5b`. A name without a tag is `<name>:latest`. |
| `auth_kind` | `anonymous` | `anonymous` only: an Ollama server is sent no credential. |
| `credential_file`, `credential`, `credential_header` | absent | Refused. |
| `model_prefix` | absent | Refused: the model's status is read for `upstream_model` alone, so the model serves its alias alone. |
| `status_probe` | absent | Refused: the model's status is read from the server's own lists. |
| `billing_kind` | `self-hosted` | As for an endpoint. |

Every refusal is `config:value` naming the key, and quotes no value. The model's status is read
from the server's own lists, never `/v1/models`: `GET /api/tags`, then `GET /api/ps` when the
tags list the model. It is `ready` when the running models list it, `stopped` `not-loaded` when
only the tags do (a call loads it), `unreachable` `model-not-pulled` when the tags do not, and
`unreachable` `no-answer` when the server does not answer. A call to a model that is not pulled is
refused `503 model-not-pulled`, its message naming the alias. The gateway never asks the server to
pull, create, copy, delete or push a model. [Serve a local Ollama
model](../guides/serve-a-local-ollama-model.md) walks through it.

```toml
[hosted.local]
kind = "ollama"
upstream_model = "qwen2.5:1.5b"
```

### The client's own credential {#client-routes}

`auth_kind = "client"` is admitted only for `base_url = "https://api.anthropic.com"`, on
`wires = ["messages"]` alone, with no static header named like a forwarded one and no
`reasoning_effort`, and only when the document's `ingress_auth` is `"none"`: the client's
`authorization` is its own credential, never an owner token. It is how Claude Code on a Claude
subscription reaches Anthropic through a local gateway:

```toml
[hosted.claude]
wires = ["messages"]
base_url = "https://api.anthropic.com"
auth_kind = "client"
billing_kind = "subscription"
tool_calling = "parsed"
model_prefix = "claude-"
```

## `[aliases.<alias>]` {#aliases}

A second name of a `[models.<alias>]` model whose Messages requests reach the pod with thinking
off. Claude Code sends an effort on every Messages request, `high` unless told otherwise and
never `none`, and the pinned vLLM turns the Qwen3.8 template's thinking on for every effort it
receives; so a model that should answer structured steps without thinking first gets a second
name instead of a second pod.

| Key | Default | Rule |
| --- | --- | --- |
| `model` | required | The alias of a `[models.<alias>]` model whose `wires` include `messages`. A hosted model, another alias, a name nothing declares or a model without the `messages` wire is `config:value` naming `aliases.<alias>.model`. |
| `effort` | required | `none`, the only value. Any other is `config:value` naming `aliases.<alias>.effort`. |

A missing key, any other key or a value that is not a string is `config:schema`. The alias
follows the provider-name rule and is not also a `[models.*]` or `[hosted.*]` alias
(`config:value` naming `aliases.<alias>`).

The alias is relayed as its model: the same wires, tool calling, windows, token price and
`start_on_request`, and sent to the pod under the model's alias. It has no pod of its own: a
call to it, its wake and its status take the model's pod, so a call to either name starts or
waits on the one pod, and a pod start is counted once. The watchdog counts a call to the alias
as activity of the model's pod, and `/v1/routes/<alias>` lists the model's targets. A model
without the `messages` wire would have the alias's Messages requests translated, and a
translated request carries no forced effort, so such an alias is refused. On the Messages wire
every request to it reaches the pod with each `output_config.effort` set to `null` and
`chat_template_kwargs` holding `"enable_thinking":false`, whatever effort or
`chat_template_kwargs` it sent; the model's own name is relayed exactly as without the alias.
Chat and Responses requests to the alias are relayed as to the model.

```toml
[aliases.qwen-plain]
model = "qwen"
effort = "none"
```

## `token_price` {#token-prices}

`[models.<alias>.token_price]` and `[hosted.<alias>.token_price]` (every kind) declare what the
model's tokens cost, in USD per 1000000 tokens. Each call's usage record then names what the
call cost, `cost_usd`, priced by llm's own pricing code; the binary writes it as the `cost_usd`
field of its `usage` event, and the record `GET /v1/calls/{id}` answers names it as `cost_usd`, a
JSON string such as `"0.000138"`, after its token keys.

| Key | Default | Rule |
| --- | --- | --- |
| `input` | required | Uncached input tokens. |
| `output` | required | Output tokens, reasoning included. |
| `cache_read` | absent | Input tokens read from the cache. |
| `cache_creation` | absent | Input tokens written to the cache. |

Each value is a quoted decimal string llm's `Amount` reads: no sign, no exponent, no leading
zero, no space, at most nine decimal places. A value outside that rule, a number that is not a
string, a missing `input` or `output` and any other key are refused `config:value` naming
`<table>.<alias>.token_price.<key>`, never quoting the value; a `token_price` that is not a table
is `config:schema`.

```toml
[hosted.remote.token_price]
input = "3"
output = "15"
cache_read = "0.3"
cache_creation = "3.75"
```

A call has a cost only when its answer ended as written, its answer reported its input and its
output tokens, and every class with a count above 0 has a rate. Otherwise `cost_usd` is absent,
never `0` and never an estimate: cache reads reported against a price without `cache_read`
leave the call without a cost, while a count of 0 in a class without a rate costs nothing.

## `[token_review]` {#token-review}

With `ingress_auth = "service-account-token"` a model call is admitted when its bearer is the
owner secret, compared first and never sent anywhere, or when the cluster's TokenReview API
says the bearer is an authenticated token for `audience` and names a service account on
`principals`. Every other path (`/metrics`, `/v1/routes`, `/dashboard`) takes the owner secret
alone. A [guide](../guides/admit-service-account-tokens.md) sets up the cluster side.

| Key | Required | Rule |
| --- | --- | --- |
| `audience` | yes | The audience the callers' projected tokens carry and the only one the review asks for: 1 to 256 bytes of printable ASCII without a space. No default: a token minted for the API server, or for anything else, is refused. |
| `principals` | yes | 1 to 64 service accounts, each `system:serviceaccount:<namespace>:<name>`, none twice. Exact names only; there is no namespace wildcard. |
| `api` | no | `https://<host>[:<port>]`, default `https://kubernetes.default.svc`. |
| `ca_file` | no | The CA bundle the API server's certificate must chain to, and the only roots trusted for the review. Read once at start: a rotated cluster CA takes a restart. Default `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt`. |
| `credential_file` | no | The gateway's own token, sent to the API server as `authorization: Bearer` and read again at every review, so a rotated token is used from the next review on. Default `/var/run/secrets/kubernetes.io/serviceaccount/token`. |

Both files are read at start through the trusted-file reader as a kubelet mounts them: symbolic
links are followed, and a file any group or other user can write is refused. A refusal is
`token-review:<rule>` and names the setting, never the path; a CA file without a usable PEM
certificate is `token-review:ca`. A token the review rejects (another audience, a principal off
the list, expired, not a token) is `401 credential-rejected`; a review that gets no answer (the
API server unreachable, a timeout, a non-2xx status, a malformed body, too many reviews at once)
is `503 credential-review-unavailable`. Neither ever admits. A token whose own claims, read
unverified, have expired or do not name `audience` is refused before any review, and an answer
that sets `status.error` is a rejection. The admitted service account is the
`principal` field of the call's usage record. The review's timeout, concurrency and cache are in
[bounds of the service-account review](https://github.com/beyond10x/llm-gateway/blob/main/docs/gateway.md#bounds-of-the-service-account-review).

```toml
listen = "0.0.0.0:8080"
ingress_auth = "service-account-token"
owner_secret_file = "/etc/llm-gateway/owner-secret"

[token_review]
audience = "llm-gateway"
principals = ["system:serviceaccount:agents:coding-agent"]
```

## `[launch]` {#launch}

What `b10x-llm-gateway claude` and `b10x-llm-gateway codex` start their agent with. Both read the
running gateway's routes from `GET /v1/launch` on its `listen` address, and both refuse a document whose
`ingress_auth` is not `"none"`.

| Key | Default | Rule |
| --- | --- | --- |
| `launch.claude.main_model` | `opus` | The `--model` Claude Code is started with unless its arguments name one; not empty, not starting with `-`. |
| `launch.claude.implementer_route` | `qwen` | The route `CLAUDE_CODE_SUBAGENT_MODEL` names. A route that does not relay the messages wire with tool calling is left out, with a note. |
| `launch.claude.agents` | absent | A JSON object passed as `--agents`. |
| `launch.claude.subagent_instruction` | below | Appended to Claude Code's system prompt with `--append-system-prompt`, `{route}` and `{model}` replaced by the implementer route and the model it serves; joined after the user's own `--append-system-prompt` text when there is one. An empty string adds nothing. |
| `launch.codex.model` | `qwen` | The route Codex is started on unless its arguments name a model. |

The default instruction:

```text
To run a sub-agent on `{route}` ({model}), call the Agent tool without the `model` parameter: CLAUDE_CODE_SUBAGENT_MODEL is `{route}` in this session. The `model` enum cannot name `{route}`. Do this whenever the user asks for a `{route}` sub-agent.
```

## `[watchdog]` {#watchdog}

The gateway can stop with a pod still running. `b10x-llm-gateway watchdog` is a second process,
reading the same document, that terminates the pods the gateway owns when the gateway cannot.
With this table `serve` writes a heartbeat file, and `watchdog` reads it.

| Key | Default | Rule |
| --- | --- | --- |
| `heartbeat_file` | `$XDG_STATE_HOME/llm-gateway/heartbeat.json`, else `$HOME/.local/state/llm-gateway/heartbeat.json` | Where `serve` writes its heartbeat: written to a temporary file beside it, mode 0600, then renamed. A relative path is read from the working directory. |
| `heartbeat_seconds` | 10 | 1 to 60: how often `serve` rewrites it. |
| `poll_seconds` | 60 | 10 to 3600: how often `watchdog` reads the Runpod account. |
| `stale_minutes` | 5 | 1 to 1440, and at least three heartbeat intervals: a heartbeat this old, or none, is a gateway that cannot stop its pods. |
| `not_ready_minutes` | 45 | 1 to 1440: a pod not ready this long is terminated. |
| `max_runtime_hours` | 24 | 1 to 720: a pod running this long is terminated. |
| `min_balance_usd` | 5 | 0 to 100000: under this account balance every owned pod is terminated. |

A value outside its range is `config:value` naming `watchdog.<key>`. The watchdog terminates a
pod only when its name starts with `b10x-llm-` and its owner tag is `b10x-llm-gateway`; no other
pod of the account is ever touched. For each such pod it terminates on the first of: the balance
under `min_balance_usd`, a stale heartbeat, the runtime past `max_runtime_hours`, the pod not
ready for `not_ready_minutes`, or the heartbeat showing its model with no call in flight and
its last call older than its `idle_timeout_minutes` (never less than `stale_minutes`).

## `[interaction_records]` {#interaction-records}

With this table the binary lifts every finished model call into agenticspec's session records
(sessions, agents, prompts, model turns with the gateway's facts, tool calls; no message text)
and appends them to one SQLite file, which `agenticspec serve` reads
([Record agent sessions](../guides/record-agent-sessions.md)).

| Key | Default | Rule |
| --- | --- | --- |
| `path` | required | The SQLite file, not empty. A relative path is read from the working directory. Its directory must exist at start, and the file must be missing, empty, only agenticspec's `meta` table without a schema row, or a session store whose schema row reads, as agenticspec writes it, the version the binary writes (5) or an earlier one, which agenticspec's open migrates, keeping its wire records; any other, or a file another process holds locked past 2 s, is `config:value` naming `interaction_records.path` and the reason (both versions, or `held locked by another process`), never the path, and the file is left as it was. Give the binary a file of its own, never the `sessions.db` agenticspec keeps for its transcripts. |
| `queue` | 1024 | 1 to 65536: finished calls waiting for the writer thread. A call that finds it full is not recorded and is counted in `llmgw_interaction_records_dropped_total`. |
| `call_bytes` | 8388608 (8 MiB) | 1 to 33554432: the most bytes of request body and answer body together copied of one call. A call over it, or whose answer was not decoded, is not recorded and is counted in `llmgw_interaction_records_skipped_total`; its answer is unchanged. |
| `sessions_held` | 4096 | 1 to 65536: sessions whose agents the binary remembers, the least recently used forgotten first. A forgotten session's next call continues under the same agent id, without the chain state of its earlier turns. |

A value outside its range is `config:value` naming `interaction_records.<key>`; any other key
is `config:schema`. A refused call is recorded as an incomplete turn; a token count is not
recorded. A call whose request the lift cannot read (it yields no step) is not recorded and is
counted in `llmgw_interaction_records_skipped_total`; a call the writer fails to store is counted
in `llmgw_interaction_records_failed_total`, with at most one warning line per second naming how
many failed since the last. At SIGINT or SIGTERM the binary waits at most 2 s for the writer,
then counts the calls still queued and those failed since the last warning on one line,
`b10x-llm-gateway: interaction records: <n> calls not stored at stop`, before its unchanged stop
line.

## `routes` {#routes}

`routes = "<path>"` names a routes document in llm's `llm.routes/1` format: endpoints, each with
a `base_url`, a `protocol` and an `auth`, and routes, each a name for `<endpoint>/<model-id>`,
optionally with its `capabilities`. The binary reads it once at start and parses it with llm
0.24.0's own parser, which reads no secret and no other file. Each route is then served as a model
under its route name, from a hosted endpoint built from its endpoint: on the endpoint's wire
alone, relayed byte for byte, and translated from the other two
([translate between wires](../guides/translate-between-wires.md#serve-the-models-of-a-routes-document)).
A changed routes document takes a restart.

| Rule | Refusal |
| --- | --- |
| The file is read through the trusted-file reader (below): at most 2 MiB, llm's own bound, and not group- or world-writable. | `config:<rule>`, its message starting `routes:` and naming the rule, never the path |
| A document llm refuses: malformed YAML, an unknown field, a repeated key, an unknown format, protocol, auth kind or secret source, a literal secret, a relative `file:` path, a route to an undeclared endpoint. | `config:value`, its message `routes <llm's message>`, which names a field, endpoint or route and never a value |
| A Bedrock endpoint (`bedrock: {region: ..}`) and a route's `aliases`, which llm admits and the gateway does not serve, are refused, never ignored: a Bedrock model is a [`kind = "bedrock"`](#bedrock-models) hosted model. | `config:value` naming `routes.<endpoint>` or `routes.<route>.aliases` |
| A route's `price` is read as llm writes it, per 1000000 tokens: `input`, `cached_input`, `cache_creation_input` and `output` are the [token price](#token-prices)'s `input`, `cache_read`, `cache_creation` and `output`, and a class it leaves out has no rate. Its `currency` is `USD`. | `config:value` naming `routes.<route>.price.currency`, quoting no value |
| A credentialed endpoint's `auth.secret` is a `file:` reference, an absolute path or `~/<path>` resolved against `HOME`, or a [`keychain:<service>/<account>`](#keychain-references) reference. An `env:` secret is refused: the gateway reads credentials from trusted files and the OS keychain only. | `config:value` naming `routes.<endpoint>.auth.secret` |
| The key file or keychain entry is read once at start, as a hosted model's `credential_file` or `credential` is. | `endpoint-credential:<rule>`, its message starting `routes.<endpoint>.auth.secret:` and naming the rule, never the path |
| `base_url` names the endpoint's API path, such as `https://api.anthropic.com/v1`: llm appends the wire path to it, and the gateway replaces the wire path's leading `/v1` with it, and the two agree only when the URL has a path. A URL without a path, a path outside the base-path rule of [`[hosted.<alias>]`](#hosted-models), or a credential over `http` to a host that is not loopback is refused. | `config:value` naming `routes.<endpoint>.base_url` |
| An `api-key` endpoint's `auth.header` is one a hosted model's `credential_header` may be, and on a `messages` endpoint it is not `anthropic-version` (in any case), the header every request to such an endpoint is sent. | `config:value` naming `routes.<endpoint>.auth.header` |
| A route name is not also a `[models.<alias>]`, `[hosted.<alias>]` or `[aliases.<alias>]` alias. | `config:value` naming `routes.<route>` |
| A route name is at most 246 bytes, though llm admits 256: its inventory target id `<route>.<wire>` must fit the gateway's 256-byte label, and `responses` is the longest wire. | `config:value` naming `routes.` and the name's first 32 bytes, and the bound |

An endpoint whose key is `secret: "env:ANTHROPIC_API_KEY"`:

```text
b10x-llm-gateway: refused config:value: routes.anthropic.auth.secret must be a file: or keychain: reference: the gateway reads a credential from a trusted file or the OS keychain, never from an env: variable
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
`model-unknown`. A route's status is its endpoint's probe, as a
[`[hosted.<alias>]`](#hosted-models) model's is, and `GET /v1/status` lists it with `kind`
`route`.

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
| `api_key_file` | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| a route endpoint's `file:` secret | at most 4096 bytes | no group or other permission at all | one non-empty printable token |
| `metrics_bearer_file` | 32 to 4096 bytes | no group or other permission at all | one printable token; trailing ASCII whitespace is trimmed |
| `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` (a Bedrock model only) | at most 16386 bytes | not group- or world-writable | one line of printable ASCII, at most 16384 bytes after one line end |

A broken rule is refused as `<source>:<rule>`, the source being `config`, `owner-secret`,
`vllm-api-key`, `endpoint-credential`, `metrics-bearer` or `container-token` (a routes document is a `config` file, a route's key file an `endpoint-credential` one).
The message names the setting that gave the path (`routes`, `owner_secret_file`,
`models.<alias>.vllm_api_key_file`, `providers.<name>.api_key_file`, `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`, ...; the document's
own refusals need none) and the rule's detail. It never quotes the path or the file a link
resolves to, so a secret written into `--config` or a path setting by mistake is not printed:

```text
b10x-llm-gateway: refused owner-secret:unsafe-mode: owner_secret_file: has mode 644; none of 077 may be set
```

[Refusals](refusals.md#startup-refusals) lists every startup code.

### Credential files {#credential-files}

A hosted model's `credential_file`, a route endpoint's `file:` secret, a provider's `api_key_file` and `metrics_bearer_file`
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
no other user can replace the file. Its success line and its refusals about the file or its
directory name the setting (`hosted.<alias>.credential_file`, `routes.<endpoint>.auth.secret`),
never the path.

### Keychain references {#keychain-references}

A gateway run by one operator on their own machine may name a credential by an OS keychain entry
instead of a file: `keychain:<service>/<account>`, split at the first `/`, each name 1 to 256
printable ASCII characters without a space. The keys that take one are a provider's `api_key`, a
model's `vllm_api_key`, a hosted model's `credential` and a route endpoint's `auth.secret`. A
gateway in a cluster names files: it never assumes a desktop keychain.

The entry is read once, at start, through the platform's own tool, which receives the two names
and nothing else: `secret-tool lookup service <service> account <account>` on Linux (libsecret,
with any Secret Service such as GNOME Keyring or KWallet), and
`security find-generic-password -s <service> -a <account> -w` on macOS. The value is read from
the tool's standard output into memory that is overwritten when it is let go, and is taken by
the token rule above: one trailing line ending is trimmed, and the rest must be one printable
ASCII token of at most 4096 bytes. An entry the keychain does not hold, a keychain the tool cannot
read (no tool, a locked keychain, a refused unlock prompt) and a value outside the token rule each
refuse the start, as `<source>:unreadable`, `<source>:too-large` or `<source>:not-a-token`, the
source being the key's own (`endpoint-credential` for `api_key`, `credential` and `auth.secret`,
`vllm-api-key` for `vllm_api_key`). The message names the document key and never the reference
or the value.

Store an entry with the same tool, reading the value from standard input so that it reaches no
argument list or shell history:

```shell
secret-tool store --label="Runpod API key" service llm-gateway account runpod
security add-generic-password -s llm-gateway -a runpod -w
```

`secret-tool store` prompts for the value on a terminal and otherwise reads it from standard
input; `security add-generic-password` with `-w` as its last option prompts for it. Both are the
tools' documented forms and are untested here.
