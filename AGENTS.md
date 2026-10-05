# AGENTS.md — llm-gateway

What this repository is for is in [README.md](README.md); this file is what an agent changing it must
know.

## Serves

This repository serves what llm's client crates call: the gateway that answers them, and the
hosting contract and adapters that provision the endpoints they reach. The client side (the neutral
turn, protocol projections, credentials, routing and cost) stays in
[beyond10x/llm](https://github.com/beyond10x/llm); change it there.

## Layout

| Path | What it is |
| --- | --- |
| `crates/llm-gateway` | `b10x-llm-gateway`. Its manifest declares no dependency, and `tests/dependency_boundary.rs` keeps it that way |
| `crates/llm-provision` | `b10x-llm-provision`, the hosting lifecycle contract; no dependency |
| `crates/llm-runpod` | `b10x-llm-runpod`, the Runpod adapter over `RunpodTransport`; only `EmulatedRunpod` exists |
| `crates/llm-modal` | `b10x-llm-modal`, exports nothing yet |
| `checks/conformance` | `b10x-llm-gateway-conformance`, the ESS conformance runner |
| `spec/` | The ESS system `llm-gateway`: domains `llm-gateway.gateway`, `llm-gateway.hosting`, `llm-gateway.runpod` |
| `contracts/` | Authored scenarios, their manifest `ess-inputs.yaml`, `baseline.json`, and the generated `suite.json` and `schema/` |
| `docs/` | The gateway and hosting contracts, which the crate tests read, and the verification records |

## Gate

`task check` runs, in order: `task rust` (`cargo test --workspace --locked`, `cargo fmt --all
--check`, Clippy with `-D warnings`), `ess specify validate --path spec` and `task conformance`
(`cargo run --locked -p b10x-llm-gateway-conformance -- check`). CI
(`.github/workflows/gate.yml`) runs the same steps on Rust 1.98.0.

The conformance runner refuses drift: regenerate `contracts/suite.json` with
`ess verify conform synthesize --path spec --suite-format 5 --target ir --scenarios contracts --out
<file>` and `contracts/schema/schema` with `ess generate --path spec --kind schema --out <dir>`.
Every scenario file under `contracts/` must be listed in `contracts/ess-inputs.yaml`.

## Specification boundary

The domains moved from llm's system `llm` and were renamed into this system (`llm.hosting` became
`llm-gateway.hosting`, and so on), because ESS requires a domain to be named inside its system.
`llm-gateway.hosting` names four entities of llm's domains (`llm.catalog.Account`,
`llm.catalog.DeploymentSpec`, `llm.budget.Ledger`, `llm.budget.Reservation`) by id. ESS has no
cross-system relation, so those edges are recorded as `BOUNDARY:` comments beside the fields that
carry the ids. Do not copy llm's domains in to make them resolve.

The records under `docs/verification/` were made in beyond10x/llm before the move and name the
scenarios by their old `llm.*` domain names.

## Dependencies on llm

A crate here that needs an llm client crate takes it by tag:
`{ git = "https://github.com/beyond10x/llm", tag = "<release>" }`, never by path. None does today.

## Rules

- Anything that runs is Rust, with clap derive for command lines.
- No test makes a paid call or provisions an external resource.
- Every commit and push is `b10x-bot[bot]`'s through `b10x-gates bot`; every GitHub write goes
  through `b10x-gates api`.
- Use a managed worktree for changes.
- No `/home/<name>/` path literals anywhere: common Gates personal-paths has no allowance.
