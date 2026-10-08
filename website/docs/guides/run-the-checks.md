---
title: Run the checks
sidebar_position: 3
description: What each stage of the repository gate checks, how to run it alone, and what a green gate does not establish.
lede: task check runs Rust, the documentation generator, the specification and conformance; none of it makes a paid call or provisions a resource.
source: Taskfile.yml, .github/workflows/gate.yml, .github/workflows/pages.yml, crates/llm-gateway-docs, checks/conformance; the outputs below are pasted from a run of task check
---

# Run the checks

```bash
task check
```

It needs [Task](https://taskfile.dev) and the `ess` command of
[ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) 0.56.0 on
`PATH`. CI's `Gate` workflow runs the same four stages on Rust 1.98.0; run it on that toolchain
(`RUSTUP_TOOLCHAIN=1.98.0 task check`), because a newer Clippy can add lints CI does not have.

## 1. Rust

```bash
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

`task rust` runs this stage alone. The workspace forbids `unsafe`, and Clippy runs `all` plus
`pedantic` at deny. No test calls Runpod or starts a pod: the Runpod tests drive `EmulatedRunpod`,
and the production transport is driven against a fixture of the `connectors` CLI and a loopback
pod.

## 2. Documentation

```bash
cargo run --locked -p llm-gateway-docs -- generate --check
```

```text
website/data/status.json: current
website/docs/reference/cli.md: current
website/docs/reference/crates.md: current
website/docs/reference/refusals.md: current
website/docs/status.mdx: current
website/docs: no raw admonition title, story id or home-directory path
```

`llm-gateway-docs` regenerates the [CLI reference](../reference/cli.md) from the binary's clap
definition, [Crates](../reference/crates.md) from `cargo metadata`,
[Refusals](../reference/refusals.md) from the gateway's refusal codes and the [Status](/docs/status)
page from its capability list, and fails when a file on disk differs. Generation fails when a
`shipped` capability names a test that no longer exists. Run
`cargo run --locked -p llm-gateway-docs -- generate` to rewrite the generated files.

The site itself builds with Node 20 or newer, and fails on a broken link:

```bash
npm --prefix website ci
npm --prefix website run build
```

## 3. Specification

```bash
ess specify validate --path spec
```

```text
llm-gateway v1 — 8 file(s), valid
```

The ESS system `llm-gateway` has seven domains under `spec/domains/`: `gateway`, `deployment`,
`telemetry`, `hosting`, `runpod`, and the planned `upstream` and `clients`, which no code
implements yet.

## 4. Conformance

```bash
cargo run --locked -p b10x-llm-gateway-conformance -- check
```

```text
{"total":201,"passed":201,"failed":0,"error":0,"unsupported":0,"skipped":0}
{"total":201,"passed":201,"failed":0,"error":0,"unsupported":0,"skipped":0}
{"total":201,"passed":201,"failed":0,"error":0,"unsupported":0,"skipped":0}
```

The runner first checks that `contracts/suite.json` and the generated schemas equal what ESS
generates from `spec/`, then runs the authored scenarios under `contracts/` against the crates
three times. It fails when the answered or total count falls below the floor in
`contracts/baseline.json` (201 of 201), when a scenario is skipped, or when the three runs
disagree. It runs only from the repository root and writes its reports to `target/conformance/`.

## What a green gate does not establish

It does not establish that the binary relays to a Runpod pod (it refuses to), that the
`connectors` transport works against the real CLI and Runpod, or that any client completes a live
session. See [Status](/docs/status).
