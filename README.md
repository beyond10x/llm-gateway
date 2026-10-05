# llm-gateway

llm-gateway is the serving side of [llm](https://beyond10x.github.io/llm/)
([GitHub](https://github.com/beyond10x/llm)): an authenticated gateway for one owner, and a hosting
contract with Runpod and Modal adapters for provisioning model endpoints. llm's client crates call a
model; this repository answers them and, later, starts the pods they reach.

**Documentation:** there is no documentation site yet. The contracts live in this tree:
[docs/gateway.md](docs/gateway.md) (the gateway's HTTP surface, refusals, lifecycle and bounds),
[docs/hosting.md](docs/hosting.md) (the owned-resource hosting lifecycle) and
[docs/llmgw-capability-matrix.md](docs/llmgw-capability-matrix.md) (what is still missing before
it can replace llmgw, row by row).

**Status: 0.1.0, unreleased, tested against in-process fakes and loopback; nothing is deployed or
qualified.**

## What it is not

It is not a model client: the neutral turn, the protocol clients, credentials, routing and cost
are llm's. The gateway library relays the owner's chat, responses and messages requests to a target
its embedding hands out, and serves health and readiness probes and a read-only route inventory;
it translates no protocol, and the binary does not relay yet, because no production transport
reaches a pod. The hosting contract opens no socket and
allocates nothing. The Runpod adapter has no production transport and runs only against the
in-process `EmulatedRunpod`, and the Modal adapter exports nothing. No test makes a paid call or
provisions a resource.

Nor does it replace llmgw yet. llmgw is the gateway this repository is meant to succeed, and it stays
in service until the capability matrix has no open gap and a cutover has been qualified.

## Crates

| Package | What it is |
| --- | --- |
| `b10x-llm-gateway` | The single-owner gateway library: owner authentication, probes, route inventory, drain and stop |
| `b10x-llm-gateway-cli` | The `b10x-llm-gateway` binary: one closed TOML deployment document, the gateway, a graceful stop on a signal |
| `b10x-llm-provision` | The hosting lifecycle contract: resource identity, leases, stop obligations, and the in-process `FakeProvider` |
| `b10x-llm-runpod` | A Runpod vLLM adapter behind that contract, with the in-process `EmulatedRunpod` |
| `b10x-llm-modal` | A Modal adapter; it exports nothing yet |

No tag has been cut and the crates are not published, so build from source. The workspace needs
Rust 1.98 or newer.

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
directory. `spec/domains/deployment.yaml` holds every key, default and range.

Both files go through a trusted-file reader. Each must be a regular file, not a symlink, owned by
you or root. The document must be at most 256 KiB and not group- or world-writable. The owner
secret must be one printable token of 32 to 4096 bytes (a trailing newline or CRLF is trimmed),
readable by the owner only. Clients send it as `Authorization: Bearer <secret>`.

The gateway answers `GET /health` and `GET /ready` without a credential; the owner can read
`GET /v1/routes` and `GET /v1/routes/<alias>`. [docs/gateway.md](docs/gateway.md) has the full
surface and every refusal code.

| Event | Line on standard error | Exit status |
| --- | --- | --- |
| Serving | `b10x-llm-gateway: listening on <address>` | none yet |
| Refused start | `b10x-llm-gateway: refused <source>:<rule>: <message>` | 1 |
| SIGINT or SIGTERM | `b10x-llm-gateway: stopped by <signal> accepted=<n> completed=<n>` | 0 |

A stop answers every request already accepted first, and takes at most twice the 10-second read
timeout however slowly a client sends or reads.

## Build and check

```bash
cargo test --workspace --locked
task check
```

`task check` adds formatting, Clippy, specification validation and the conformance suite. It
needs [Task](https://taskfile.dev) and the `ess` command from
[ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) on `PATH`.
[AGENTS.md](AGENTS.md) explains each step.

## License

Apache-2.0. See [LICENSE](LICENSE).
