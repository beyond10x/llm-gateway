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
[ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) 0.77.0 on
`PATH`. CI's `Gate` workflow runs the same five stages on Rust 1.98.0; run it on that toolchain
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

## 4. Specification diff

```bash
cargo run --locked -p b10x-llm-gateway-conformance -- diff
```

```text
comparing spec/ with spec/ at 6db89ae7af7ca5b3076da3e8a9791ed7b6c428b8 (merge base of origin/main and HEAD)
llm-gateway v1 → v1
  before  8f967aba724761aaced53b8f52ccc17bac94ff7c6c04c101eb933b937bf78531
  after   8f967aba724761aaced53b8f52ccc17bac94ff7c6c04c101eb933b937bf78531

no semantic change: these two revisions mean the same system
gate --fail-on breaking-or-unknown: passed (0 acknowledged)
```

`task spec-diff` runs this stage alone. It compares `spec/` with the specification at the merge
base of `origin/main` and `HEAD` (`--base` names another revision; CI names the pull request's
base branch, or the previous head of `main` on a push) using
`ess verify diff --fail-on breaking-or-unknown`. A change ESS classifies as breaking, such as a
removed enum variant, or as unknown, such as a changed enum attribute value, fails the stage with
exit 4 and is named on a `spec diff failed on:` line. An added domain, type or variant is
compatible and passes.

To ship a change the stage fails, review the delta and acknowledge it in
`contracts/diff-acknowledgements.json`, bound to the `before` and `after` digests the run printed:

```json
{
  "format": "ess-diff-acknowledgements/1",
  "before": "<before digest>",
  "after": "<after digest>",
  "acknowledged": ["system/llm-gateway/unclassified-changed"]
}
```

ESS refuses, with exit 1, a file bound to any other pair of digests, naming an id the delta does
not hold, or naming an id twice. The file is applied only when the change ships it: a copy the
base already holds belongs to an earlier comparison.

## 5. Conformance

```bash
cargo run --locked -p b10x-llm-gateway-conformance -- check
```

```text
{"total":210,"passed":210,"failed":0,"error":0,"unsupported":0,"skipped":0}
{"total":210,"passed":210,"failed":0,"error":0,"unsupported":0,"skipped":0}
{"total":210,"passed":210,"failed":0,"error":0,"unsupported":0,"skipped":0}
```

The runner first checks that `contracts/suite.json` and the generated schemas equal what ESS
generates from `spec/`, then runs the authored scenarios under `contracts/` against the crates
three times. It fails when the answered or total count falls below the floor in
`contracts/baseline.json` (210 of 210), when a scenario is skipped, or when the three runs
disagree. It runs only from the repository root and writes its reports to `target/conformance/`.

## What a green gate does not establish

It does not establish that the binary relays to a Runpod pod (it refuses to), that the
`connectors` transport works against the real CLI and Runpod, or that any client completes a live
session. See [Status](/docs/status).
