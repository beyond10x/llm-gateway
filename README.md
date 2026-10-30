# llm-gateway

llm-gateway is the serving side of [llm](https://beyond10x.github.io/llm/)
([GitHub](https://github.com/beyond10x/llm)): an authenticated gateway for one owner, and a hosting
contract with Runpod and Modal adapters for provisioning model endpoints. llm's client crates call a
model; this repository answers them and, later, starts the pods they reach.

**Documentation:** <https://beyond10x.github.io/llm-gateway/>, built from
[`website/`](website/): [getting started](https://beyond10x.github.io/llm-gateway/docs/getting-started/),
[set up a client](https://beyond10x.github.io/llm-gateway/docs/guides/set-up-a-client/),
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

**Status: 0.18.0, released 2026-10-30 as a source release with the gateway image; tested
against in-process fakes and loopback; nothing is deployed or qualified.**
[CHANGELOG.md](CHANGELOG.md) lists every release and what is unreleased.

## What it is not

It is not a model client: the neutral turn, the protocol clients, credentials, routing and cost
are llm's. The gateway library relays the owner's chat, responses and messages requests to a target
its embedding hands out, and serves health and readiness probes, a public model listing with
setup instructions per client, and a read-only route inventory;
it translates no protocol itself, but holds the port through which the binary translates a
request on a wire the model's upstream does not speak, within llm 0.19.0's published subset
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
Identity), carrying a messages client's `anthropic-beta` names to InvokeModel only when the model's `anthropic_beta` list allows them; that path has run only against a fixture Bedrock Runtime, never against AWS. The hosting contract opens no socket and allocates nothing. The Runpod
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
| `b10x-llm-runpod` | A Runpod vLLM adapter behind that contract: the in-process `EmulatedRunpod`, and `ConnectorsRunpod`, which reaches Runpod through the `connectors` CLI |
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
`owner_secret_file`.
`spec/domains/deployment.yaml` holds every key, default and range.

Every file the binary reads goes through a trusted-file reader. It opens the file once, without
following a symlink in its last component, and makes every check on that one open file; a file
that grows past its bound after the check is still refused. Each must be a regular file, not a
symlink, owned by you or root, and UTF-8 (`<source>:not-utf8`). The document must be at most
256 KiB and not group- or world-writable. The owner secret must be one printable token of 32 to
4096 bytes (trailing ASCII whitespace is trimmed), with no group or world permission at all
(mode & 0o077 is 0). Clients send it as `Authorization: Bearer <secret>`.

A `[models.<alias>]` table may name `vllm_api_key_file`: the file holding the key that model's
pod's vLLM server expects, the same value stored as the model's Runpod secret. It is read once at
startup under the owner secret's rules, except that any non-empty printable token of at most 4096
bytes is accepted; a broken rule refuses the start as `vllm-api-key:<rule>`. The document holds no
Runpod API key and refuses `runpod_api_key_file`: that key stays in connectors' keyring
([docs/design/runpod-clients.md](docs/design/runpod-clients.md) § 4, D1).

A `[hosted.<alias>]` table serves a model from a hosted endpoint instead of a pod: `wires`,
`base_url` (`http://` or `https://`, a `host[:port]` and an optional base path that replaces
the wire path's leading `/v1`, so `https://openrouter.ai/api/v1` asks `/api/v1/chat/completions`),
`auth_kind` (`anonymous`,
`bearer` or `api-key`), `credential_file` unless anonymous, `credential_header` for `api-key`,
and optionally `billing_kind`, `upstream_model`, `context_window`, `tool_calling` and a
`[hosted.<alias>.headers]` table of static non-secret headers such as `anthropic-version`. The
credential file is read once at startup under the owner secret's rules (`endpoint-credential:<rule>`);
a `bearer` endpoint is sent `authorization: Bearer <credential>`, an `api-key` endpoint the
credential in its named header. An `https` endpoint is verified against the same compiled-in
roots as a pod. A credential goes over plain `http` only to a loopback host (`127.0.0.0/8`,
`::1`, `localhost`); any other `http` endpoint that is not `anonymous` is refused as
`config:value`.

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
`connectors` is relayed to its Runpod pod at `https://<pod id>-8000.proxy.runpod.net/v1/` over
TLS, with the pod's certificate verified against the Mozilla roots compiled into the binary before
any request byte is sent; no document key or option adds a root or turns verification off. A
model whose provider declares no `connectors` is answered `503` `target-unavailable`.

The command line is `--config <file>`, `--help` and `--version`, and nothing else; any other
argument is refused with exit status 2.

`RUST_LOG` sets the level of the structured log events the process writes to standard error
beside the lines below (default `info`). At the default level those events are one `usage` event
per authenticated model call; `RUST_LOG=warn` leaves only the lines below.

| Event | Line on standard error | Exit status |
| --- | --- | --- |
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
cargo test --workspace --locked
task check
```

`task check` adds formatting, Clippy, the documentation drift check, specification validation and
the conformance suite. It needs [Task](https://taskfile.dev) and the `ess` command from
[ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) on `PATH`.
[AGENTS.md](AGENTS.md) explains each step.

Build the documentation site with Node 20 or newer:

```bash
npm --prefix website ci
npm --prefix website run build
```

## License

Apache-2.0. See [LICENSE](LICENSE).
