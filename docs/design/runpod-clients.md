# Design: a Runpod-hosted model for Claude Code, Codex and Loom

Status: proposed, 2026-10-08. Design and plan only: nothing here is built, no pod has been started
and no Runpod API has been called. The nouns are declared, marked `PLANNED`, in
[`spec/domains/clients.yaml`](../../spec/domains/clients.yaml). The work is `epic:client-access`
in the AEP store.

## The request

> draft a design which would allow me to use a runpod hosted model within claude, codex and loom

Operator input during the design, 2026-10-08:

> technically we might want to offer runpod via connectors (add openapi + little config there) -
> or maybe llm only depends on secrets to access those secretly ...

That input, and a second one on provider descriptions in llm, is design choice D1 below.

## The shape

```text
Claude Code ──messages──┐
Codex ──────responses───┼──► b10x-llm-gateway ──same wire, vLLM key──► vLLM pod on Runpod
Loom (llm) ──any wire───┘    one owner bearer        (relayed, untranslated)
                             model registry
                             cold-start hold, idle reap
```

One `b10x-llm-gateway` process runs on the operator's workstation, listening on loopback. Each
client sends the owner's one bearer credential and names a model by its deployment alias. The
gateway relays the request on the client's own wire to the pod serving that alias, starting the pod
on the first request and terminating it after it has been idle. It translates nothing: a client is
served exactly when the model declares the client's wire, and vLLM serves all three
(llmgw `docs/model-profiles.md`: both proven profiles declare `["chat", "responses", "messages"]`).

This is llmgw's shape. llmgw already serves "Claude Code (via `ANTHROPIC_BASE_URL`) and Codex (via a
`config.toml` profile)" (llmgw `README.md:16`) from a Runpod pod, but without authentication
(llmgw `README.md:27-29`). llm-gateway is its successor, and the design adds nothing to the relay
that llmgw did not have except the owner credential, which llm-gateway already enforces, and one
early refusal (tool calling, below).

## What exists and what is missing

Every row of the chain below is a story already in the store, except the five this design adds.

| Link | State | Story |
| --- | --- | --- |
| Owner bearer authentication, three wire paths, the untranslated relay | implemented | `story:gateway-auth`, `story:wire-relay` |
| The binary reads one closed document and serves | implemented, but it does not relay: no production transport reaches a pod (`README.md:22-23`) | `story:gateway-binary` |
| Runpod key and vLLM key read at startup | draft | `story:provider-key-files` |
| The binary relays through `RunpodPool`, sending the vLLM key | draft | `story:gateway-deployment` |
| Runpod REST create, list, terminate, uptime, readiness | draft | `story:runpod-production-transport` |
| The binary starts real pods | draft | `story:live-runpod-wiring` |
| The binary reaches a pod's proxy URL over TLS | **missing**: `RelayTarget::connect` leaves TLS to the embedding (`docs/gateway.md` "The relay"), and no story gives the binary a TLS client for `https://<pod>-8000.proxy.runpod.net` (`docs/hosting.md:326-330`) | **`story:pod-proxy-tls`** (new) |
| A cold request waits for its pod; the reaper runs on a timer; orphans swept at startup | draft | `story:cold-start-hold` |
| `GET /` setup instructions, `GET /v1/models` | draft | `story:public-model-listing` |
| A model declares tool calling; a tool request to a model without it is refused before waking a pod | **missing** | **`story:model-tool-calling`** (new) |
| The messages wire answers what Claude Code 2.1.293 sends | **missing** | **`story:claude-code-wire`** (new) |
| One recorded live session per client against a real pod | **missing** | **`story:client-qualification`** (Claude Code, Codex) and **`story:loom-qualification`** (new) |

## 1. Claude Code

Facts, from the Claude Code documentation read 2026-10-08 and the installed `claude` 2.1.293:

| Fact | Source |
| --- | --- |
| `ANTHROPIC_BASE_URL` sends API requests to a gateway | https://code.claude.com/docs/en/env-vars |
| `ANTHROPIC_AUTH_TOKEN` is sent as `Authorization: Bearer <value>`; `ANTHROPIC_API_KEY` as `X-Api-Key` | https://code.claude.com/docs/en/authentication |
| `apiKeyHelper` runs a command whose output is sent in both `Authorization` and `x-api-key` | https://code.claude.com/docs/en/llm-gateway-connect |
| A gateway must serve the Anthropic Messages format: `POST /v1/messages` required, `POST /v1/messages/count_tokens` optional (a character estimate is used without it) | https://code.claude.com/docs/en/llm-gateway-protocol |
| Inference is `POST /v1/messages?beta=true`; a gateway matches on the path | same |
| A startup probe `HEAD /api/hello` is sent | same |
| Streaming must be `text/event-stream`, unbuffered, every event through `message_stop`, `ping` events kept | same |
| For a model id it does not recognise, Claude Code assumes a 200K window unless `CLAUDE_CODE_MAX_CONTEXT_TOKENS` is set, and sends `thinking: {"type":"adaptive"}` | same; https://code.claude.com/docs/en/model-config |
| `ANTHROPIC_MODEL`, `ANTHROPIC_DEFAULT_OPUS_MODEL`, `_SONNET_MODEL`, `_HAIKU_MODEL` (the Haiku one also serves background tasks) and `CLAUDE_CODE_SUBAGENT_MODEL` name the models | https://code.claude.com/docs/en/model-config, https://code.claude.com/docs/en/env-vars |
| `API_TIMEOUT_MS` defaults to 600000 | https://code.claude.com/docs/en/env-vars |
| `--settings <file-or-json>` loads an extra settings file for one session | `claude --help` (2.1.293) |
| Anthropic does not support routing Claude Code to non-Claude models through any gateway | https://code.claude.com/docs/en/llm-gateway |

What the gateway must answer, and where it stands:

| Claude Code sends | The gateway today | Change |
| --- | --- | --- |
| `Authorization: Bearer <owner token>` | admitted (`docs/gateway.md` "The relay", step 1) | none. `ANTHROPIC_API_KEY` must not be used: `x-api-key` is read as no credential (`credential-absent`) |
| `POST /v1/messages?beta=true` | the query is discarded (`crates/llm-gateway/src/server.rs:421-422`), so it is the messages wire | none |
| `model: <alias>` on every request, background ones included | `model-unknown` for anything that is not an alias | none: every model setting names the alias (§ 5) |
| `anthropic-version`, `anthropic-beta` headers | not forwarded: the outbound head carries `host`, `content-type`, `content-length`, `connection` only (`crates/llm-gateway/src/relay.rs:582-587`) | `story:claude-code-wire` records whether vLLM needs either; the documentation's forward-verbatim rule is written for an Anthropic upstream |
| `output_config.effort: high` | relayed as `xhigh` (matrix row W4), because the served chat template rejects `high` as a 500 that Claude Code retries (llmgw `README.md:79-85`) | none |
| `thinking: {"type":"adaptive"}` | relayed | `story:claude-code-wire`: relayed or mapped, settled from a recorded vLLM answer (`llm-gateway.clients.ThinkingHandling`) |
| `HEAD /api/hello` | an unauthenticated `HEAD` off the two probe paths is `credential-absent` 401 (`docs/gateway.md` "The HTTP surface") | `story:claude-code-wire` records what Claude Code does with that answer, and changes the gateway only if Claude Code fails on it |
| `POST /v1/messages/count_tokens` | `path-unknown` 404 after authentication | none: Claude Code falls back to an estimate |
| tools on every request | relayed; vLLM parses tool calls only when started with `--enable-auto-tool-choice` and a parser (llmgw `docs/model-profiles.md`) | `story:model-tool-calling` |
| a stream that lasts as long as the model talks | relayed chunk by chunk, no total deadline (`docs/gateway.md` "Lifecycle") | none |

## 2. Codex

Facts about the installed `codex` 0.160.0:

| Fact | Source |
| --- | --- |
| A provider is `[model_providers.<id>]` with `name`, `base_url`, `env_key`, `wire_api`, `request_max_retries` (default 4), `stream_max_retries` (default 5), `stream_idle_timeout_ms` (default 300000); `openai`, `ollama` and `lmstudio` are reserved ids | https://learn.chatgpt.com/docs/config-file/config-reference |
| `wire_api = "responses"` is the only supported value; `"chat"` is a hard error in this build ("`wire_api = "chat"` is no longer supported") | same; the string inside the 0.160.0 binary; https://github.com/openai/codex/pull/10157 |
| `model`, `model_provider`, `model_context_window`, `model_auto_compact_token_limit` | config reference |
| `--profile <name>` layers `$CODEX_HOME/<name>.config.toml` over the user configuration | `codex exec --help` (0.160.0) |
| Each turn is `POST {base_url}/responses`, `stream: true`, with the `env_key` value as `Authorization: Bearer` | openai/codex rust-v0.160.0 `codex-rs/codex-api/src/endpoint/responses.rs`, `codex-rs/model-provider/src/bearer_auth_provider.rs` |
| The body always carries `tools` (shell, `apply_patch`, MCP tools), `store: false`, `reasoning` and `include: ["reasoning.encrypted_content"]` | rust-v0.160.0 `codex-rs/codex-api/src/common.rs`, `codex-rs/core/src/client.rs` |
| A plain `env_key` provider does not fetch `GET {base_url}/models` | inferred from rust-v0.160.0 `codex-rs/models-manager/src/manager.rs`, not observed |

So Codex reaches the gateway on the responses wire, with `base_url` the gateway origin plus `/v1`.
The gateway needs no change for it beyond tool calling. What is not known is whether vLLM's
`/v1/responses` accepts everything Codex sends: `include: reasoning.encrypted_content`, replayed
reasoning items, and `apply_patch` as a free-form `custom_tool_call`. Only a live pod can answer
that, which is `story:client-qualification`. If vLLM refuses one of them, that is a translation
question (`story:gateway-translation`), not something to patch in the relay.

## 3. Loom

| Fact | Source |
| --- | --- |
| A model is `trait Model` with `turn(&TurnRequest, &mut dyn StreamSink, &Cancel)` | beyond10x/llm 0.3.1 `crates/llm-core/src/port.rs:43-51` |
| Three clients, `ChatClient`, `ResponsesClient`, `MessagesClient`, selected by `Protocol` | `crates/llm-chat/src/client.rs:18`, `crates/llm-responses/src/client.rs:52`, `crates/llm-messages/src/client.rs:36`, `crates/llm-core/src/vocabulary.rs:4-10` |
| A catalog `llm.catalog/1` declares a `bearer` account, an endpoint `base_url` and a serving model with `protocol` and `capabilities.tools`, `context_window` | `crates/llm-routing/src/catalog.rs:39-61`, `crates/llm-providers/src/declaration.rs:114-193`, `examples/catalog.toml:14-87` |
| `base_url` must carry `/v1`: llm appends `chat/completions`, `responses` or `messages` | `crates/llm-providers/src/declaration.rs:45-52` |
| A bearer secret resolves from a file (`FileResolver`, Linux, strict modes) or an environment variable | `crates/llm-credentials/src/file.rs:13-21`, `crates/llm-credentials/src/environment.rs:13-21` |
| Tool definitions and tool calls cross all three clients | `crates/llm-core/src/turn.rs:77-88`, `crates/llm-chat/src/outgoing.rs:30-45`, `crates/llm-responses/src/request.rs:83-103`, `crates/llm-messages/src/codec.rs:38-50` |
| Loom's `run` takes any `&dyn Model` | beyond10x/loom `crates/loom-intake-slice/src/run.rs:252-259` |
| The `b10x-loom` command line builds only `codex_model`, a responses client against the Codex subscription backend, with a fixed 400000-token window | beyond10x/loom `crates/loom-cli/src/main.rs:145,202`; beyond10x/llm `crates/llm-tool-call/src/lib.rs:47-55,147-217` |
| llm has no factory from a `Protocol` to a client; each caller writes the `match` | `crates/llm-routing/src/fallback.rs:22-24`, `crates/llm-docs/examples/local_endpoint.rs:25-34` |

So llm can already describe the route: a `bearer` account, an endpoint
`base_url = "http://127.0.0.1:8080/v1"`, and a serving model whose `upstream_name` is the gateway
alias. The gateway needs nothing for it. Loom cannot select that route yet. Two changes outside
this repository close it, sent as needs:

- **llm**: a function that builds the `Model` a catalog serving model declares, matching its
  `Protocol` onto the three clients and its account onto a credential resolver. Without it, every
  consumer repeats the `match` of `local_endpoint.rs:30-34`.
- **loom**: a `b10x-loom` option that names an llm catalog file and a route alias, used for
  `--model` and `--classifier-model` instead of `codex_model`.

The recommended wire for Loom is `chat`. It is the one llm's clients and vLLM have both served
longest, and it is the one llmgw's proven profiles answered for harness and metaharness (llmgw
`README.md:15`). That is a recommendation, not a measurement.

## 4. The gateway between the clients and Runpod

**Translation.** None. Each wire is relayed to the same path at the pod (`docs/gateway.md` "The
relay", steps 6-9). vLLM's OpenAI-compatible server answers all three paths, so Claude Code meets
vLLM's `/v1/messages`, Codex meets `/v1/responses`, and Loom whichever its serving model declares.

**One owner token.** `owner_secret_file` (`spec/domains/deployment.yaml`, `DeploymentConfiguration`)
holds one token of at least 32 bytes. All three clients send it as `Authorization: Bearer`. It never
reaches a pod: the outbound head carries only the vLLM key (`story:gateway-deployment` acceptance 3).

**The model registry.** The deployment document's `[models.<alias>]` tables
(`llm-gateway.deployment.ModelDeclaration`). Each alias is what every client puts in `model`. The
owner reads it at `GET /v1/routes`; `story:public-model-listing` adds the unauthenticated
`GET /v1/models` and `GET /`, which renders one `llm-gateway.clients.ClientProfile` per client:
the settings of § 5 with the alias and context window filled in. It never wakes a pod.

**Cold start.** `story:cold-start-hold`: a request for a model whose pod is not serving is held up
to a hold budget, then answered `503` with `Retry-After`. The budget must end before the client
gives up. Codex's `stream_idle_timeout_ms` defaults to 300000 and Claude Code's `API_TIMEOUT_MS` to
600000 (§ 1, § 2), so the recommended hold is **240 s** for these clients. That is inferred: neither
source says whether the timeout runs before the response head arrives. A longer start is covered by
the client's own retries of a `503` (Codex: `request_max_retries`, default 4), and the pod keeps
starting meanwhile; the startup deadline (`start_wait_seconds`, default 600) is what terminates a
pod that never serves (`docs/hosting.md:302`).

**Idle reap.** `idle_timeout_minutes`, default 30, terminates a pod with no request in flight, never
below the measured cold start and never during a stream (`docs/hosting.md:303`, matrix rows L10-L12).
`story:cold-start-hold` puts the reaper on a 60 s timer (row L13) and runs the orphan sweep before
serving (row L15). Runpod never terminates a pod by itself (llmgw `README.md:106-107`), so these two
are the billing bound.

**A model that cannot call tools.** Claude Code and Codex send tools on every request. A pod
started without `--enable-auto-tool-choice` and a `--tool-call-parser` cannot answer them as tool
calls. `story:model-tool-calling` adds a per-model `tool_calling` declaration
(`llm-gateway.clients.ToolCalling`) and the refusal `tools-not-served` (400), decided with
`wire-not-served` before any target is asked for, so such a request never starts a billed pod. A
request without tools is relayed as today, so a text-only model still serves a plain chat client.
`GET /` offers no Claude Code or Codex profile for a model whose tool calling is `Absent`.

**Context window.** The gateway does not know the client's window; the client must be told. The
model's `context_window` is published by `GET /v1/routes` today and is what § 5 copies into
`CLAUDE_CODE_MAX_CONTEXT_TOKENS`, `model_context_window` and `capabilities.context_window`. Claude
Code's system prompt and tool definitions take part of it; which of llmgw's profiles (32768 or 65536
tokens) leaves enough room is measured by `story:client-qualification`, not assumed here.

**Keep-alive.** The gateway answers one request per connection (`docs/gateway.md` "The HTTP
surface"). All three clients open a new connection when the old one closes; this costs a loopback
handshake per request and nothing else.

### D1: where Runpod is described, how its API is reached, and where the keys live

The gateway needs two secrets, the Runpod API key (to create, list and terminate pods) and each
model's vLLM key (sent to the pod as a bearer), and it needs to know Runpod's API and the URL a pod
serves at. Today all of that is written into `crates/llm-runpod`. The operator proposed two other
places for it during this design:

> technically we might want to offer runpod via connectors (add openapi + little config there) -
> or maybe llm only depends on secrets to access those secretly ...

> in llm we have crates/llm-providers there we have packages for each provider or JUST a yaml file
> describing that provider. what wire formats and urls they support, which models, etc - this is
> then being used from both sides (connectors, loom)

Read 2026-10-08: beyond10x/llm 0.3.1 `crates/llm-providers` holds `auth.rs`, `binding.rs` and
`declaration.rs`, and its `Provider` is an `id` and a `category` (`declaration.rs:114-117`). There
is no per-provider description yet. Runpod publishes an OpenAPI 3.0.3 document,
`https://rest.runpod.io/v1/openapi.json`, with `POST /pods`, `GET /pods`, `GET /pods/{podId}` and
`DELETE /pods/{podId}`; it reports no uptime and no restart count, only `lastStartedAt`. connectors
v0.30.0 serves an HTTP API from a pinned OpenAPI document through its catalog provider, with no
adapter code, and keeps the credential in its keyring (connectors `README.md`;
`docs/local-catalog-provider.md`).

| | A: provider description in llm, control plane through connectors | B: own client, keys from `secrets` | C: own client, keys from files (current plan) |
| --- | --- | --- | --- |
| Where Runpod is described | once, in llm: a provider description document in `crates/llm-providers` naming the control-plane API (the pinned OpenAPI source and the four operations) and the inference side (the pod URL pattern `https://<pod>-8000.proxy.runpod.net`, the wires vLLM serves, `bearer` auth) | `crates/llm-runpod` | the same as B |
| Who reads it | connectors compiles the control-plane half into its Runpod bundle; llm and Loom build routes from the inference half; this gateway's Runpod adapter reads both | this repository only | the same as B |
| Runpod API calls | connectors' catalog provider; `crates/llm-runpod`'s transport invokes it | an HTTP and TLS client in `crates/llm-runpod` | the same as B |
| Runpod API key | in connectors' keyring; the gateway never holds it | beyond10x/secrets (`secrets-keychain` locally, `secrets-remote` in a cluster) | a trusted file (`story:provider-key-files`) |
| vLLM key | a trusted file: the relay needs the value on every request | from `secrets` | a trusted file |
| `Refused` versus `Lost` on a create, the double-billing guard (`docs/hosting.md`, "GPU choice") | connectors classifies a write `refused` only on a documented definite refusal and `unknown` otherwise (connectors `docs/local-catalog-provider.md` "Invoke"), the same rule | written and tested here (`story:runpod-production-transport` acceptance 2) | the same as B |
| Crash-loop detection | `lastStartedAt` moving forward stands in for llmgw's GraphQL uptime query (inferred, unverified) | GraphQL uptime query, as llmgw | the same as B |
| Work outside this repository | llm: the provider description format and Runpod's description; connectors: compile the bundle from it, with an approval policy for unattended writes ("an approval policy naming them"); a connectors owner process runs beside the gateway | none; secrets crates taken by tag | none |
| Work removed here | the HTTP and TLS client for the API in `story:runpod-production-transport`; the hard-coded proxy URL in `crates/llm-runpod/src/provider.rs` | none | none |

**Recommendation: A.** It is the operator's proposal, and it is the only option in which Runpod's
API and URLs are written down once for every consumer instead of once per repository. It also keeps
the account credential out of the gateway process, and connectors already draws the
`refused`/`unknown` line this repository would otherwise implement itself. Its cost is two upstream
changes, in llm and connectors, before `story:runpod-production-transport` can start, and a
connectors owner process beside the gateway. B's `secrets` stays useful under A for the one
secret the gateway still holds, the vLLM key; that is a later choice and not on this path. With A,
`story:runpod-production-transport` and `story:provider-key-files` are rewritten before they
start: the transport invokes connectors, and the key file story reads only the vLLM key. The
`decision-blocker:runpod-control-plane` in the store holds this choice. Default if not answered: A.

## 5. What the operator does by hand

1. **Runpod account and key.** Create the account at https://www.runpod.io, add credit, and create an
   API key in the console (Settings, API Keys). Store it where D1 says: A, through `connectors` (the
   connect step for the Runpod connection, once connectors carries the bundle); C, as a file:
   `install -m 600 /dev/stdin ~/.config/b10x-llm-gateway/runpod-api-key`, pasting the key.
2. **The pod's vLLM key.** Create a Runpod secret (console, Secrets) named for the model, e.g.
   `vllm_small`, holding `openssl rand -hex 32`, and write the same value to
   `~/.config/b10x-llm-gateway/vllm-key-small` with mode 600. The pod receives it as
   `{{ RUNPOD_SECRET_vllm_small }}` (`crates/llm-runpod/src/request.rs:116-117`).
3. **The gateway token.** `openssl rand -hex 32 > ~/.config/b10x-llm-gateway/owner-secret && chmod 600 ~/.config/b10x-llm-gateway/owner-secret`.
4. **The deployment document** `~/.config/b10x-llm-gateway/gateway.toml`: `listen = "127.0.0.1:8080"`,
   `owner_secret_file`, the key files, and one `[models.<alias>]` block copied from llmgw's
   `docs/model-profiles.md` with `wires = ["chat", "responses", "messages"]` and
   `tool_calling = "parsed"` (the key `story:model-tool-calling` adds). Run `b10x-llm-gateway --config ~/.config/b10x-llm-gateway/gateway.toml`
   (the binary relays only once the stories of the table above are implemented).
5. **Claude Code**, in its own settings file so other sessions keep their own endpoint:
   `~/.config/b10x-llm-gateway/claude-settings.json`

   ```json
   {
     "apiKeyHelper": "cat ~/.config/b10x-llm-gateway/owner-secret",
     "env": {
       "ANTHROPIC_BASE_URL": "http://127.0.0.1:8080",
       "ANTHROPIC_MODEL": "small",
       "ANTHROPIC_DEFAULT_OPUS_MODEL": "small",
       "ANTHROPIC_DEFAULT_SONNET_MODEL": "small",
       "ANTHROPIC_DEFAULT_HAIKU_MODEL": "small",
       "CLAUDE_CODE_SUBAGENT_MODEL": "small",
       "CLAUDE_CODE_MAX_CONTEXT_TOKENS": "65536",
       "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"
     }
   }
   ```

   Start it with `claude --settings ~/.config/b10x-llm-gateway/claude-settings.json`.
6. **Codex**, as a profile layered over the user configuration: `~/.codex/runpod.config.toml`

   ```toml
   model = "small"
   model_provider = "b10x-gateway"
   model_context_window = 65536

   [model_providers.b10x-gateway]
   name = "b10x llm-gateway"
   base_url = "http://127.0.0.1:8080/v1"
   env_key = "B10X_GATEWAY_TOKEN"
   wire_api = "responses"
   ```

   Start it with `B10X_GATEWAY_TOKEN="$(cat ~/.config/b10x-llm-gateway/owner-secret)" codex --profile runpod`.
7. **Loom**, once the two needs of § 3 are released: an llm catalog with a `bearer` account whose
   `secret_reference_id` the `FileResolver` maps to `~/.config/b10x-llm-gateway/owner-secret`, an
   endpoint `base_url = "http://127.0.0.1:8080/v1"`, and a serving model with `protocol = "chat"`,
   `upstream_name = "small"`, `capabilities.tools = true` and `capabilities.context_window = 65536`;
   then `b10x-loom` with the new catalog and route option.

`small` stands for the alias the document declares. `GET /` prints steps 5 to 7 with the alias and
window filled in once `story:public-model-listing` lands.

## 6. Cost

Runpod rates read from https://www.runpod.io/pricing on 2026-10-08 (on-demand pods, per hour):

| GPU | Community | Secure |
| --- | --- | --- |
| L40S | $0.79 | $1.09 |
| RTX A6000 | $0.33 | $0.53 |
| A40 | $0.35 | $0.49 |
| H100 NVL | $2.59 | $3.19 |

The deployment default is `cloud_type = SECURE` (`spec/domains/deployment.yaml`, `CloudType`). The
pricing page shows per-second rates. Storage: container disk $0.10/GB/month; network volume
$0.07/GB/month (same page).

| Item | Cost | Source |
| --- | --- | --- |
| One idle hour, llmgw's default profile on an L40S, Secure | $1.09, plus 80 GB container disk at $0.10/GB/month ≈ $0.011 | rate above; `disk_gb` default 80 (`deployment.yaml`) |
| One idle hour, the H100 NVL profile, Secure | $3.19 | rate above |
| The idle tail every session ends with | `idle_timeout_minutes` × rate: 30 min = $0.55 (L40S) or $1.60 (H100 NVL), Secure | default 30 (`deployment.yaml`) |
| One cold start | its duration × rate. **I don't know the duration**: no measurement is recorded here or in llmgw. Its ceiling is the startup deadline: llmgw's profiles set `startWaitSeconds = 1800`, so at most $0.55 (L40S) or $1.60 (H100 NVL) | llmgw `docs/model-profiles.md` |
| A network volume for the weights (optional) | 60 GB × $0.07 = $4.20 per month, standing | llmgw `docs/model-profiles.md:101`; rate above |
| Tokens | none: a pod bills by the hour, not by the token | — |

What bounds spend:

| Bound | Where |
| --- | --- |
| Idle reap after `idle_timeout_minutes` | `docs/hosting.md:303`; scheduled by `story:cold-start-hold` (row L13) |
| A pod that never serves is terminated at its startup deadline | `docs/hosting.md:302` |
| A crash-looping pod is terminated and refused | `docs/hosting.md:301` |
| One pod per model: a second cold request waits on the first start | matrix row L5 |
| Orphan sweep: pods carrying this controller's tag that no record holds | `docs/hosting.md:304`; at startup by `story:cold-start-hold` (row L15) |
| `HostingPolicy` resource ceiling `max_active` and time ceiling `max_lifetime_ms` | `docs/hosting.md:205-216`; `story:gateway-deployment` acceptance 4 decides where they come from |
| A refusal before any target is asked for never wakes a pod (`model-unknown`, `wire-not-served`, `tools-not-served`) | `docs/gateway.md` "The relay", step 5 |
| The Runpod account balance: a prepaid account cannot spend past its credit | the operator's account; I have not read Runpod's billing terms, so whether a pod is stopped at zero balance is unverified |

## 7. Later: models set through the gateway's API

Operator input, 2026-10-08:

> and runpod is also just a provider, it offers the same thing, but it has some control api around
> - start stop install model, etc ... how crazy would it be: start llm-gateway, configure runpod,
> then being able to set via api model, llvm params, etc ...

Not part of this epic; recorded here as the milestone after it. Design choice D5 is decided: A, the
operator, 2026-10-08, a later epic drafted after `story:client-qualification`
(`decision-blocker:gateway-model-api`). Today the model
registry is the `[models.<alias>]` tables of a closed document read once at startup
(`spec/domains/deployment.yaml`), and the route inventory is an immutable snapshot
(`docs/gateway.md` "The route inventory"). The proposal splits the document in two: the document
keeps what the process needs to start (listen address, owner secret, providers and their keys,
spending ceilings), and models become owner-authenticated commands on the running gateway.

| Command | Effect |
| --- | --- |
| declare a model: alias, Hugging Face model, image, GPU types, context window, vLLM arguments, wires, tool calling | validated by the rules `ModelDeclaration` has today (`config:value` becomes the command's refusal); the next cold start uses it |
| change a model | a running pod of the old declaration is replaced through the hosting controller, which already retires and replaces deployments (`docs/hosting.md:303`, `cleanups` `replaced=[..]` in `spec/domains/runpod.yaml`) |
| retire a model | its pod is stopped through the controller; requests answer `model-unknown` |
| start or stop a model's pod ahead of use | `RunpodPool::ensure` and a stop, which today only a request or the reaper trigger |

Why it is not far off: the hosting contract already keeps durable records, generations and stop
obligations per deployment (`docs/hosting.md`), so a declaration that changes at runtime is a new
generation, not a new mechanism. It also suits the specification better than the document does:
commands with outcomes and a lifecycle are what ESS synthesises scenarios for, while the document is
observed only by process tests (`spec/domains/deployment.yaml`, `ESS-LIMIT`).

What it costs:

- the owner credential can then choose GPU types, so it can choose what is billed. The document must
  carry the ceilings that bound it: the allowed GPU types and `HostingPolicy`'s `max_active` and
  `max_lifetime_ms` (`docs/hosting.md:205-216`);
- the registry must survive a restart: declarations become durable records beside the hosting
  records;
- the route inventory becomes a sequence of snapshots, swapped whole, and `config_digest` names the
  snapshot rather than the document bytes;
- "install a model" is vLLM downloading the weights at pod start; a network volume caches them
  (llmgw `docs/model-profiles.md` "Weight cache").

Under D1 option A, Runpod's control operations come from the provider description in llm, so this
command surface is the gateway's own and needs no Runpod code beyond what the transport already
uses.

## Open design choices

| | Question | Options | Recommendation |
| --- | --- | --- | --- |
| D1 | Where Runpod is described, how its API is reached, where the keys live | A provider description in llm plus connectors, B `secrets`, C files | A (above) |
| D2 | `HEAD /api/hello` | A leave it `credential-absent`; B answer it as an unauthenticated probe | A unless `story:claude-code-wire` records a failure |
| D3 | Claude Code's `thinking: adaptive` | A relay; B map on the messages wire, as W4 does | decided by `story:claude-code-wire` from a recorded vLLM answer |
| D4 | Where the gateway runs | A the workstation, loopback; B a server, which needs TLS on the listener and a public endpoint | A: no listener TLS, nothing exposed, and every client named here runs on the workstation |
| D5 | Models and vLLM parameters set through an owner API on the running gateway (§ 7) | A a later epic, after `story:client-qualification`; B now, replacing the document's `[models]` before the relay chain is built | Decided: A, the operator, 2026-10-08 (`decision-blocker:gateway-model-api`) |
