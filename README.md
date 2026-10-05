# llm-gateway

The serving side of [llm](https://github.com/beyond10x/llm): an authenticated, single-owner gateway
and the hosting contract with its Runpod and Modal adapters. llm's client crates call a model
endpoint; this repository is what serves and provisions one. It moved here from beyond10x/llm with
its history, tests and specification.

**Status: libraries tested against in-process fakes; nothing is deployed and nothing is
qualified.** The gateway authenticates one owner and serves probes and a read-only route
inventory; it does not translate a protocol or proxy a model call yet. The hosting contract opens
no socket and allocates nothing. The Runpod adapter runs only against an in-process emulator, and
the Modal adapter exports nothing. No test makes a paid call or provisions a resource.

## Crates

| Package | What it is |
| --- | --- |
| `b10x-llm-gateway` | The authenticated single-owner gateway: probes, route inventory, drain and stop ([docs/gateway.md](docs/gateway.md)) |
| `b10x-llm-provision` | The hosting lifecycle contract: resource identity, leases, stop obligations, and an in-process `FakeProvider` ([docs/hosting.md](docs/hosting.md)) |
| `b10x-llm-runpod` | A Runpod vLLM adapter behind that contract, with the in-process `EmulatedRunpod` |
| `b10x-llm-modal` | A Modal adapter; it exports nothing yet |

The crates depend on no llm client crate. A crate that needs one takes it from
`https://github.com/beyond10x/llm` at a release tag, never by path.

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
