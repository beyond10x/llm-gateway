# llm-gateway

llm-gateway is the serving side of [llm](https://beyond10x.github.io/llm/)
([GitHub](https://github.com/beyond10x/llm)): an authenticated gateway for one owner, and a hosting
contract with Runpod and Modal adapters for provisioning model endpoints. llm's client crates call a
model; this repository answers them and, later, starts the pods they reach.

**Documentation:** <https://beyond10x.github.io/llm-gateway/>, built from
[`website/`](website/): [getting started](https://beyond10x.github.io/llm-gateway/docs/getting-started/),
[set up a client](https://beyond10x.github.io/llm-gateway/docs/guides/set-up-a-client/),
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

**Status: 0.6.0, released 2026-10-08 as a source release; tested against in-process fakes and
loopback; nothing is deployed or qualified.** [CHANGELOG.md](CHANGELOG.md) lists every release
and what is unreleased.

## What it is not

It is not a model client: the neutral turn, the protocol clients, credentials, routing and cost
are llm's. The gateway library relays the owner's chat, responses and messages requests to a target
its embedding hands out, and serves health and readiness probes, a public model listing with
setup instructions per client, and a read-only route inventory;
it translates no protocol, and the binary does not relay to a Runpod pod yet: it refuses a
document that declares a `connectors` provider until it can reach the pod's `https` endpoint
(story:pod-proxy-tls). The hosting contract opens no socket and
allocates nothing. The Runpod adapter has a production transport that reaches Runpod only through the
`connectors` CLI and that no binary uses yet; its tests run against `EmulatedRunpod` or a fixture
of that CLI, and the Modal adapter exports nothing. No test makes a paid call or
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

Releases are source releases tagged `<version>` (the first is `0.1.0`); the crates are not
published, so build from source. The workspace needs Rust 1.98 or newer.

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
directory. At least one `[models.<alias>]` table is required (`config:value` otherwise).
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

The gateway answers `GET /health`, `GET /ready`, `GET /v1/models` (each model with its
`max_model_len` and wires) and `GET /` (the Codex, Claude Code or Loom settings for each model,
chosen by `User-Agent`) without a credential; the owner can read
`GET /v1/routes`, `GET /v1/routes/<alias>` and `GET /metrics`, llmgw's 13 `llmgw_` counters as
Prometheus text. [docs/gateway.md](docs/gateway.md) has the full surface and every refusal code.
The binary also takes the owner's `POST /v1/chat/completions`, `POST /v1/responses` and
`POST /v1/messages` for the wires each model declares, but relays none of them to a pod yet: a
model whose provider declares no `connectors` is answered `503` `target-unavailable`, and a
document in which a provider declares `connectors` is refused at startup until the binary can
reach the pod's `https` endpoint (story:pod-proxy-tls).

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
