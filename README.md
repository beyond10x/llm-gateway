# llm-gateway

llm-gateway is the serving side of [llm](https://beyond10x.github.io/llm/)
([GitHub](https://github.com/beyond10x/llm)): an authenticated gateway for one owner, and a hosting
contract with Runpod and Modal adapters for provisioning model endpoints. llm's client crates call a
model; this repository answers them and, later, starts the pods they reach.

**Documentation:** <https://beyond10x.github.io/llm-gateway/>, built from
[`website/`](website/): [getting started](https://beyond10x.github.io/llm-gateway/docs/getting-started/),
[set up a client](https://beyond10x.github.io/llm-gateway/docs/guides/set-up-a-client/),
[a local gateway for Claude Code](https://beyond10x.github.io/llm-gateway/docs/guides/local-gateway-for-claude-code/),
[translate between wires](https://beyond10x.github.io/llm-gateway/docs/guides/translate-between-wires/),
[CLI reference](https://beyond10x.github.io/llm-gateway/docs/reference/cli/),
[deployment document](https://beyond10x.github.io/llm-gateway/docs/reference/deployment-document/),
[crates](https://beyond10x.github.io/llm-gateway/docs/reference/crates/) and
[status](https://beyond10x.github.io/llm-gateway/docs/status/).
The contracts live in this tree:
[docs/gateway.md](docs/gateway.md) (the gateway's HTTP surface, refusals, lifecycle and bounds),
[docs/hosting.md](docs/hosting.md) (the owned-resource hosting lifecycle) and
[docs/llmgw-capability-matrix.md](docs/llmgw-capability-matrix.md) (what is still missing before
it can replace llmgw, row by row). [docs/model-profiles.md](docs/model-profiles.md) holds two proven
model declarations, and the root `Dockerfile` builds a distroless, non-root image of the binary.

**Status: 0.26.0, released 2026-11-26 as a source release with the gateway image; tested
against in-process fakes and loopback; nothing is deployed or qualified.**
[CHANGELOG.md](CHANGELOG.md) lists every release and what is unreleased.

## What it is not

It is not a model client: the neutral turn, the protocol clients, credentials, routing and cost
are llm's. The gateway library relays the owner's chat, responses and messages requests to a target
its embedding hands out, and serves health and readiness probes, a public model listing with
setup instructions per client, and a read-only route inventory;
it translates no protocol itself, but holds the port through which the binary translates a
request on a wire the model's upstream does not speak, within llm 0.24.0's published subset
([docs/gateway.md](docs/gateway.md), "Translation"): a field outside the subset is refused, never
dropped. Claude Code's first turn and Codex's first turn are carried to another wire with their
controls (effort, thinking, parallel tool calls, cache and verbosity hints). Codex's later turns
are served too: the gateway hands a Responses client the upstream's reasoning state sealed for
the route and binds it on the next turn of that route only; Claude Code's later turns, which
replay reasoning another upstream minted, are still refused. The binary relays to a Runpod pod at its `https` proxy URL over
TLS, but that path has run only against a fixture of the `connectors` CLI and a loopback pod,
never against Runpod. It also relays a model declared as `[hosted.<alias>]` to a hosted endpoint
at an `http` or `https` base URL; that path has run only against loopback endpoints, never
against a vendor API. A `[hosted.<alias>] kind = "bedrock"` model is translated and sent to
Amazon Bedrock with llm's Bedrock client, signed from the container credentials endpoint (Amazon EKS Pod
Identity), carrying a messages client's `anthropic-beta` names to InvokeModel only when the model's `anthropic_beta` list allows them and a client's reasoning effort as `output_config.effort` when the model's `reasoning_efforts` holds it (refused otherwise, never dropped); that path has run only against a fixture Bedrock Runtime, never against AWS. A `[hosted.<alias>] kind = "ollama"` model is relayed to an Ollama server's own chat, responses and messages paths, its status read from the server's lists of pulled and loaded models, and never pulled by the gateway; that path has run only against a fake Ollama on loopback, never against an Ollama server. The hosting contract opens no socket and allocates nothing. The Runpod
adapter's production transport reaches Runpod only through the `connectors` CLI; its tests run
against `EmulatedRunpod` or a fixture of that CLI, and the Modal adapter exports nothing. No test makes a paid call or
provisions a resource.

Nor does it replace llmgw yet. llmgw is the gateway this repository is meant to succeed, and it stays
in service until the capability matrix has no open gap and a cutover has been qualified.

## Crates

| Package | What it is |
| --- | --- |
| `b10x-llm-gateway` | The single-owner gateway library: owner authentication, probes, the public model listing and setup instructions, route inventory, drain and stop |
| `b10x-llm-gateway-cli` | The `b10x-llm-gateway` binary: one closed TOML deployment document, the gateway, a graceful stop on a signal |
| `b10x-llm-provision` | The hosting lifecycle contract: resource identity, leases, stop obligations, and the in-process `FakeProvider` |
| `b10x-llm-runpod` | A Runpod vLLM adapter behind that contract, on Secure Cloud on-demand pods of the `vllm/vllm-openai` image only: the in-process `EmulatedRunpod`, `ConnectorsRunpod`, which reaches Runpod through the `connectors` CLI, and `RestRunpod`, which calls Runpod's REST API with a key the process holds |
| `b10x-llm-modal` | A Modal adapter; it exports nothing yet |
| `llm-gateway-docs` | Generates the documentation site's CLI, crate and refusal references and its status page, checks them for drift, and binds a built site to its commit |

Releases are tagged `<version>` (the first is `0.1.0`); the crates are not published, so build
from source. The workspace needs Rust 1.98 or newer. From the first release after 0.9.0, each
release tag also publishes the gateway image, built from the root `Dockerfile` for `linux/amd64`
and `linux/arm64`, as `ghcr.io/beyond10x/llm-gateway:<version>`; its
`org.opencontainers.image.revision` label names the tag's commit. The ghcr package is private
when it is first created: a cluster needs an image pull secret until the package's visibility is
made public.

## Run the gateway

Build the binary in a clone:

```bash
git clone https://github.com/beyond10x/llm-gateway
cd llm-gateway
cargo build --release --locked -p b10x-llm-gateway-cli
gateway="$PWD/target/release/b10x-llm-gateway"
```

In a directory of your own, write `gateway.toml`. This is the smallest useful document:

```toml
listen = "127.0.0.1:8080"
owner_secret_file = "owner-secret"

[providers.runpod]
kind = "runpod-vllm"

[models.small]
provider = "runpod"
wires = ["chat", "responses"]
context_window = 65536
hf_model = "example/small-model"
image = "vllm/vllm-openai:v0.27.1"
gpu_types = ["NVIDIA L40S"]
max_model_len = 65536
```

Create the owner secret beside it, start the gateway, and ask it something:

```bash
openssl rand -hex 32 > owner-secret
chmod 600 owner-secret
"$gateway" --config gateway.toml &

curl -s http://127.0.0.1:8080/health
curl -s -H "Authorization: Bearer $(cat owner-secret)" http://127.0.0.1:8080/v1/routes
kill -TERM %1
```

`/health` answers `{"status":"live"}`. `/v1/routes` answers one route, `small`, with the targets
`small.chat` and `small.responses`, under a `config_digest` that is the SHA-256 of
`gateway.toml`'s bytes. Without the `Authorization` header it answers `401` with the code
`credential-absent`. Standard error shows:

```text
b10x-llm-gateway: listening on 127.0.0.1:8080
b10x-llm-gateway: stopped by SIGTERM accepted=2 completed=2
```

### The deployment document

The document is closed: an unknown key at any level is refused as `config:schema`, naming the line
and the keys that are allowed. A relative `owner_secret_file` is resolved from the working
directory. At least one model, of a `[models.<alias>]` or `[hosted.<alias>]` table or the
`routes` document, is required (`config:value` otherwise). `ingress_auth = "none"` serves model
calls without a credential, for a gateway behind something that authenticates already;
`owner_secret_file` is then optional, the counters and the route inventory still take the owner
secret (and refuse everything without one), and the binary writes a warning line before it
listens. `bearer`, the default, requires
`owner_secret_file`. `ingress_auth = "service-account-token"` admits a model call with the owner
secret or with a Kubernetes service-account token the cluster's TokenReview API admits for the
`[token_review]` table's `audience` and `principals`; the admitted service account is the call's
`principal` in its usage record, and every other path still takes the owner secret alone.
`spec/domains/deployment.yaml` holds every key, default and range.

Every file the binary reads goes through a trusted-file reader. It opens the file once, without
following a symlink in its last component, and makes every check on that one open file; a file
that grows past its bound after the check is still refused. Each must be a regular file, not a
symlink, owned by you or root, and UTF-8 (`<source>:not-utf8`). A refusal names the setting and
the rule, never the path, so a secret written into a path setting by mistake is not printed. The
document must be at most
256 KiB and not group- or world-writable. The owner secret must be one printable token of 32 to
4096 bytes (trailing ASCII whitespace is trimmed), with no group or world permission at all
(mode & 0o077 is 0). Clients send it as `Authorization: Bearer <secret>`.

A `[models.<alias>]` table may name `vllm_api_key_file`: the file holding the key that model's
pod's vLLM server expects, the same value stored as the model's Runpod secret. It is read once at
startup under the owner secret's rules, except that any non-empty printable token of at most 4096
bytes is accepted; a broken rule refuses the start as `vllm-api-key:<rule>`. The document holds no
Runpod API key and refuses `runpod_api_key_file`: with a `connectors` table that key stays in
connectors' keyring, and a gateway on an operator's own machine may instead name it as
`[providers.<name>] api_key = "keychain:<service>/<account>"`, an OS keychain entry read once at
start, or as `api_key_file`, the path of a file the operator fills with the key, read once at
start under the rules of `vllm_api_key_file` (`endpoint-credential:<rule>` otherwise), and then
calls Runpod's REST API v2 itself, at `https://api.runpod.io/v2` unless the
provider names another `management_url`
([docs/design/runpod-clients.md](docs/design/runpod-clients.md) § 4, D1). The same reference
names a model's `vllm_api_key`, a hosted model's `credential` and a route endpoint's
`auth.secret` in place of a file
([keychain references](https://beyond10x.github.io/llm-gateway/docs/reference/deployment-document/#keychain-references)).

An `[aliases.<alias>]` table, `model = "<a [models.<alias>] on the messages wire>"` and
`effort = "none"`, gives that model a second name served by the same pod, whose Messages
requests reach it with thinking off whatever effort they name
([aliases](https://beyond10x.github.io/llm-gateway/docs/reference/deployment-document/#aliases)).

A `[hosted.<alias>]` table serves a model from a hosted endpoint instead of a pod: `wires`,
`base_url` (`http://` or `https://`, a `host[:port]` and an optional base path that replaces
the wire path's leading `/v1`, so `https://openrouter.ai/api/v1` asks `/api/v1/chat/completions`),
`auth_kind` (`anonymous`,
`bearer` or `api-key`), `credential_file` unless anonymous, `credential_header` for `api-key`,
and optionally `billing_kind`, `upstream_model`, `context_window`, `tool_calling`,
`status_probe` (`models`, the default, reads `<base path>/models`; `count-tokens`, for a
Messages endpoint with no model list, sends one `POST <base path>/messages/count_tokens` of a
one-word message, which asks for no generation) and a
`[hosted.<alias>.headers]` table of static non-secret headers such as `anthropic-version`. The
credential file is read once at startup under the owner secret's rules (`endpoint-credential:<rule>`);
a `bearer` endpoint is sent `authorization: Bearer <credential>`, an `api-key` endpoint the
credential in its named header. An `https` endpoint is verified against the same compiled-in
roots as a pod. A credential goes over plain `http` only to a loopback host (`127.0.0.0/8`,
`::1`, `localhost`); any other `http` endpoint that is not `anonymous` is refused as
`config:value`.
A table with `kind = "ollama"` needs nothing more: `base_url` defaults to
`http://127.0.0.1:11434`, `wires` to all three, and no credential is sent
([serve a local Ollama model](https://beyond10x.github.io/llm-gateway/docs/guides/serve-a-local-ollama-model/)).

To store a hosted endpoint's key, run
`b10x-llm-gateway credential set --config gateway.toml --model <alias>` (or `--route <name>` for
a route of the routes document). It reads the key from a hidden prompt, or from standard input
with `--stdin`, probes `GET <base path>/models` with it unless `--no-probe` (`401` or `403`
stores nothing), and writes the file the document names, mode `0600`, read back through the
gateway's own reader first. The next start reads it. The
[Store an upstream key](https://beyond10x.github.io/llm-gateway/docs/guides/store-an-upstream-key/)
guide walks through it.

A top-level `routes = "<path>"` names an llm `llm.routes/1` document instead, or as well. Each
route is served under its route name from its endpoint: relayed byte for byte on the endpoint's
wire, translated from the other two. The file is read once at start (at most 2 MiB, not group- or
world-writable); a credentialed endpoint's `auth.secret` must be a `file:` key file (`~/` is
resolved against `HOME`), and its `base_url` must name the API path, such as
`https://api.anthropic.com/v1`. The rules are on the
[deployment document](https://beyond10x.github.io/llm-gateway/docs/reference/deployment-document/#routes)
page.

An `[interaction_records]` table names a SQLite file (`path`; its directory must exist) that the
binary appends every finished model call to as
[agenticspec](https://github.com/beyond10x/agenticspec)'s session records, with no message text,
on one writer thread behind a queue of `queue` calls (default 1024); `agenticspec serve` reads it.
A call over `call_bytes` (default 8 MiB) is counted skipped and a call that finds the queue full
dropped, never stored. Without the table nothing is copied and no file is created
([record agent sessions](https://beyond10x.github.io/llm-gateway/docs/guides/record-agent-sessions/)).

The gateway answers `GET /health`, `GET /ready`, `GET /v1/models` (each model with its
`max_model_len` and wires) and `GET /` (the Codex, Claude Code or Loom settings for each model,
chosen by `User-Agent`) without a credential; the owner can read
`GET /v1/routes`, `GET /v1/routes/<alias>` and `GET /metrics`, llmgw's 13 `llmgw_` counters as
Prometheus text, with a latency histogram `llmgw_request_duration_seconds{model}` and an outcome
counter `llmgw_requests_total{model,outcome}` per model. `metrics_listen` binds a second listener
that serves the same text at `GET /metrics` and nothing else without the owner secret, behind the
optional scrape token of `metrics_bearer_file`, so a Prometheus server never holds the owner
secret; without that token it is open, so bind it only where a network policy guards it. [docs/gateway.md](docs/gateway.md) has the full surface and every refusal code.
The binary also takes the owner's `POST /v1/chat/completions`, `POST /v1/responses` and
`POST /v1/messages` for the wires each model declares. A model whose provider declares
`connectors` is relayed to its Runpod pod at `https://<pod id>-8000.proxy.runpod.net/v1/` (or
the provider's `proxy_url`, filled) over TLS, every call but a token count sent streamed and
answered whole to a client that did not stream, with the pod's certificate verified against the Mozilla roots compiled into the binary before
any request byte is sent; no document key or option adds a root or turns verification off. A
model call starts no pod: while the pod is stopped it is answered `503` `model-stopped`, and the
owner starts it with `POST /v1/models/<alias>/wake`, unless the model sets
`start_on_request = true` under its provider's `daily_cap_usd`, which needs
`price_per_hour_usd` on every model the document declares under `[models]`. Under a spend cap the
gateway keeps its own pods' estimated spend in the provider's `spend_ledger_file` (by default
`spend-ledger.json` in `$XDG_STATE_HOME/llm-gateway`, else `$HOME/.local/state/llm-gateway`), and
each cap counts the larger of Runpod's reported spend, which lags by hours, and that ledger, so
pods already stopped and pods started before a restart count. A model whose provider
declares no `connectors` is answered `503` `target-unavailable`.

The command line is `--config <file>`, `--help` and `--version`, and nothing else; any other
argument is refused with exit status 2.

`RUST_LOG` sets the level of the structured log events the process writes to standard error
beside the lines below (default `info`). At the default level those events are one `usage` event
per authenticated model call, whose field `call` is the id its answer carried in
`x-llm-gateway-call`; `GET /v1/calls/<id>` with the owner secret reads that call's record
([`docs/gateway.md`](docs/gateway.md#call-ids-and-call-records)). `RUST_LOG=warn` leaves only the
lines below.

| Event | Line on standard error | Exit status |
| --- | --- | --- |
| Metrics listener bound, once, before `listening on`, when the document sets `metrics_listen` | `b10x-llm-gateway: metrics listening on <address>`, the address it bound | none yet |
| Serving | `b10x-llm-gateway: listening on <address>` | none yet |
| Refused start | `b10x-llm-gateway: refused <source>:<rule>: <message>` | 1 |
| SIGINT or SIGTERM | `b10x-llm-gateway: stopped by <signal> accepted=<n> completed=<n>` | 0 |

`<source>` is `config`, `owner-secret`, `vllm-api-key`, `listen` or `signal`: a listen address
that cannot be bound is refused as `listen:bind`, and signal handlers that cannot be installed as
`signal:install`.

A stop answers every request already accepted first. For the probes and inspection it takes at
most twice the 10-second read timeout however slowly a client sends or reads; a request to a wire
path can hold it longer, because its head, its body, its answer and the read-out of a refusal each
have their own deadline, and a relayed stream is waited for until it ends
([docs/gateway.md](docs/gateway.md#lifecycle)).

## Build and check

```bash
task test
task check
```

`task test` runs every test with [cargo-nextest](https://nexte.st/) (on `PATH`), each in a process of its own:
a test over 60 seconds is killed and fails, the run fails over 120 seconds, and it prints the ten
slowest tests. `task check` adds formatting, Clippy, the documentation drift check, specification validation,
the specification diff against the merge base with `origin/main` and the conformance suite. It needs [Task](https://taskfile.dev) and the `ess` command from
[ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) on `PATH`.
[AGENTS.md](AGENTS.md) explains each step.

Build the documentation site with Node 20 or newer:

```bash
npm --prefix website ci
npm --prefix website run build
```

## License

Apache-2.0. See [LICENSE](LICENSE).
