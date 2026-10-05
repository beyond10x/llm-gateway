# llm-gateway

The serving side of [llm](https://github.com/beyond10x/llm): an authenticated, single-owner gateway
and the hosting contract with its Runpod and Modal adapters. llm's client crates call a model
endpoint; this repository is what serves and provisions one. It moved here from beyond10x/llm with
its history, tests and specification.

**Status: libraries and the `b10x-llm-gateway` binary tested against in-process fakes and loopback;
nothing is deployed and nothing is qualified.** The gateway authenticates one owner and serves
probes and a read-only route inventory; it does not translate a protocol or proxy a model call yet.
The hosting contract opens no socket and allocates nothing. The Runpod adapter runs only against an
in-process emulator, and the Modal adapter exports nothing. No test makes a paid call or
provisions a resource.

## Crates

| Package | What it is |
| --- | --- |
| `b10x-llm-gateway` | The authenticated single-owner gateway: probes, route inventory, drain and stop ([docs/gateway.md](docs/gateway.md)) |
| `b10x-llm-provision` | The hosting lifecycle contract: resource identity, leases, stop obligations, and an in-process `FakeProvider` ([docs/hosting.md](docs/hosting.md)) |
| `b10x-llm-runpod` | A Runpod vLLM adapter behind that contract, with the in-process `EmulatedRunpod` |
| `b10x-llm-modal` | A Modal adapter; it exports nothing yet |
| `b10x-llm-gateway-cli` | The `b10x-llm-gateway` binary: one closed TOML deployment document, the gateway, a graceful stop |

The crates depend on no llm client crate. A crate that needs one takes it from
`https://github.com/beyond10x/llm` at a release tag, never by path.

## How to run

```bash
cargo build --release --locked -p b10x-llm-gateway-cli
target/release/b10x-llm-gateway --config gateway.toml
```

The deployment document is closed TOML: an unknown key at any level is refused. The smallest one:

```toml
listen = "127.0.0.1:8080"
owner_secret_file = "/etc/llm-gateway/owner-secret"

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

The keys, defaults and ranges are llmgw's; `spec/domains/deployment.yaml` lists every rule.
Both files are read through a trusted-file reader. Each must be a regular file, not a symlink,
owned by you or root. The document must be at most 256 KiB and not group- or world-writable. The
owner secret must be one printable token of 32 to 4096 bytes, readable by the owner only (`chmod
600`). Clients send it as `Authorization: Bearer <secret>`.

The gateway answers `GET /health` and `GET /ready` without a credential. The owner can read
`GET /v1/routes` and `GET /v1/routes/<alias>`. It does not relay model calls or start pods yet
(see [docs/llmgw-capability-matrix.md](docs/llmgw-capability-matrix.md)).

Standard error gets one line per event:

| Event | Line | Exit status |
| --- | --- | --- |
| Serving | `b10x-llm-gateway: listening on <address>` | — |
| Refused start | `b10x-llm-gateway: refused <source>:<rule>: <message>` | 1 |
| SIGINT or SIGTERM | `b10x-llm-gateway: stopped by <signal> accepted=<n> completed=<n>`, after every request in flight is answered | 0 |

## Build and check

```bash
cargo test --workspace --locked
task check
```

`task check` runs the workspace tests, `cargo fmt --check`, Clippy with `-D warnings`,
`ess specify validate --path spec` and the conformance runner, which runs every authored scenario
under `contracts/` against the real crates three times. It needs [Task](https://taskfile.dev) and
[ESS](https://github.com/beyond10x/ess).

The previous gateway, [llmgw](https://github.com/beyond10x/llmgw), stays in service until a
qualified, reversible cutover.

## License

Apache-2.0. See [LICENSE](LICENSE).
