# AGENTS.md — llm-gateway

[README.md](README.md) says what this repository is and how to run it. This file is what an agent
changing it must know.

## What it owns

This repository owns the serving half of llm: the gateway that answers llm's clients, the hosting
contract, and the provider adapters that would provision the endpoints those clients reach. The
client half (the neutral turn, protocol clients, credentials, routing, cost) belongs to
[beyond10x/llm](https://github.com/beyond10x/llm); change it there, never by copying it in.

| Path | What it is |
| --- | --- |
| `crates/llm-gateway` | `b10x-llm-gateway`, the gateway library. Declares no dependency |
| `crates/llm-gateway-cli` | `b10x-llm-gateway-cli`, the `b10x-llm-gateway` binary: the closed TOML document, the trusted-file reader, the signal-driven stop. It carries the dependencies the gateway crate must not have |
| `crates/llm-provision` | `b10x-llm-provision`, the hosting lifecycle contract. Declares no dependency |
| `crates/llm-runpod` | `b10x-llm-runpod`, the Runpod adapter over `RunpodTransport`; `EmulatedRunpod` is the only transport |
| `crates/llm-modal` | `b10x-llm-modal`, exports nothing yet |
| `checks/conformance` | `b10x-llm-gateway-conformance`, the ESS conformance runner |
| `spec/` | The ESS system `llm-gateway` with seven domains: `llm-gateway.gateway`, `llm-gateway.hosting`, `llm-gateway.runpod`, `llm-gateway.deployment`, and the `PLANNED` `llm-gateway.telemetry`, `llm-gateway.upstream` and `llm-gateway.clients`, which no code implements and no scenario observes yet |
| `contracts/` | Authored scenarios, their manifest `ess-inputs.yaml`, `baseline.json`, and the generated `suite.json` and `schema/` |
| `docs/` | The gateway and hosting contracts, the llmgw capability matrix, dated verification records, and designs under `docs/design/` |
| `.engineering/` | The AEP planning store and per-story evidence |

## Serves

- **O1 — governed reach.** The gateway is a single-owner, authenticated relay whose refusal codes
  and bounds are published in `docs/gateway.md`, and hosting runs only behind the lease contract
  with stop obligations in `docs/hosting.md`.
- **O5 — the generic agent platform.** It serves the model endpoints that agent clients (Claude
  Code, Codex, Loom) reach, starting with a Runpod-hosted model, as `docs/design/runpod-clients.md`
  proposes.

## Invariants

Each claim below fails a named check when broken. Change the claim and its check together, or not
at all.

| Claim | Held by |
| --- | --- |
| The gateway crate links nothing that can resolve a secret, reach an upstream or provision (`b10x-llm-provision`, `b10x-llm-runpod`, `b10x-llm-modal`, `reqwest`, `tokio`, …), directly or transitively | `crates/llm-gateway/tests/dependency_boundary.rs` |
| Every refusal code and numeric bound in `docs/gateway.md` matches the code, and every bound flips at exactly the published number | `crates/llm-gateway/tests/gateway.rs` and `tests/adversary_pass_2.rs`, which `include_str!` the document |
| Every scenario file under `contracts/` is listed in `contracts/ess-inputs.yaml` | `every_authored_scenario_is_declared` in `checks/conformance/src/gate.rs` |
| `contracts/suite.json` and `contracts/schema/schema` equal what ESS generates from `spec/` | the drift step of `b10x-llm-gateway-conformance check` |
| The suite answers at least 179 of at least 179 scenarios and skips none, with equal counts on three consecutive runs | `contracts/baseline.json`, enforced by the same command |
| No `unsafe` code; Clippy `all` and `pedantic` are errors | `[workspace.lints]` in `Cargo.toml`, `task rust` |
| No test makes a paid call or provisions an external resource | no production transport exists; adding one is `story:runpod-production-transport` and must keep its tests on the emulator |

## Gate

`task check` runs, in order:

1. `task rust`: `cargo test --workspace --locked`, `cargo fmt --all --check`, then
   `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.
2. `ess specify validate --path spec`.
3. `task conformance`: `cargo run --locked -p b10x-llm-gateway-conformance -- check`. It refuses to
   run anywhere but the repository root, and writes its reports to `target/conformance/` under the
   root regardless of `CARGO_TARGET_DIR`.

Each step runs alone with the command shown. CI (`.github/workflows/gate.yml`) runs the same steps
on Rust 1.98.0 and uploads `target/conformance` as `serving-conformance`; the repository has no
`rust-toolchain.toml`. Run the gate on that toolchain (`RUSTUP_TOOLCHAIN=1.98.0 task check`): a
newer Clippy can add lints CI does not have. The tree also lints clean on 1.99.0 (stable on
2026-10-05). `.github/workflows/shared-gates.yml` runs the common
Gates checks against the `B10X_GATES_POLICY` secret.

Every build writes the worktree's own `target/`; never set `CARGO_TARGET_DIR`. Check `df -h /`
before a full gate. End a tree with `worktree finish --discard-cache --archive <tree>`, which
removes the build cache it recognises, instead of deleting `target/` by hand.

## Generated files

Never edit these by hand; change `spec/` or the scenarios and regenerate:

| File | Command |
| --- | --- |
| `contracts/suite.json` | `ess verify conform synthesize --path spec --suite-format 5 --target ir --scenarios contracts --out contracts/suite.json` |
| `contracts/schema/schema/` | `ess generate --path spec --kind schema --out contracts/schema` |

`ess` on `PATH` is what `task check` uses. CI installs the `ess` 0.52.0 release asset, checked
against the release's `SHA256SUMS` and a pinned SHA-256 (`.github/workflows/gate.yml`), and
`checks/conformance/Cargo.toml` pins `ess-conformance` and `ess-primitives` to tag `0.52.0`. Move
all three together, to the newest ESS release (workspace rule), and regenerate both files in the
same commit.

A new scenario goes into `contracts/ess-inputs.yaml` as well as onto disk. The run fails when the
answered or total count falls below its floor in `contracts/baseline.json`, or skipped rises above
its ceiling (`checks/conformance/src/main.rs`).

## Specification boundary

The domains came from llm's system `llm` and were renamed into this system (`llm.hosting` became
`llm-gateway.hosting`, and so on), because ESS requires a domain to be named inside its system.
`llm-gateway.hosting` names four entities of llm's domains by id: `llm.catalog.Account`,
`llm.catalog.DeploymentSpec`, `llm.budget.Ledger` and `llm.budget.Reservation`. ESS has no
cross-system relation, so those edges are `BOUNDARY:` comments beside the fields in
`spec/domains/hosting.yaml`. Do not copy llm's domains in to make them resolve.

## Dependencies on llm

A crate that needs an llm client crate takes it by tag,
`{ git = "https://github.com/beyond10x/llm", tag = "<release>" }`, never by path. None does today.

## Documents

`docs/gateway.md` and `docs/hosting.md` are contracts: a behaviour change updates them in the same
commit, and the gateway tests fail if `docs/gateway.md` drifts from the code. The capability matrix
cites llmgw and this repository at fixed commits; `story:gateway-binary` re-cited the rows it
closed (C1, C2, C4, C5, K29, K30, D2) at its own tree, and `story:wire-relay` the rows it closed
(R6, R7, R8, W1, W2, W3, W4, W5, W7, K11) at its own. `docs/verification/*` are dated records made
in beyond10x/llm before the move, some under the old `llm.*` domain names; leave them as written and
add a new record instead.

There is no documentation site and no `website/`. Creating one follows the workspace `docs` skill
(`~/beyond10x/.agents/skills/docs/SKILL.md`). The README links `docs/` until then. A change to a
command, crate or rule updates README.md and this file in the same commit.

## Planning and waves

The AEP store is `.engineering/planning/` (`aep plan artifact list`; `aep plan artifact validate`
must pass). Implemented stories have evidence under `.engineering/evidence/story/<story>/`;
adversary passes are `review-result` artifacts. Draft stories are planned work, not shipped.

A wave opens with a `plan: open wave <date>-w<NN> (<story>)` commit, implements each story on
`impl/<story>` (the failing test first, `test: … (red)`, then `feat:` and `fix:`), merges it into
`wave/<date>-w<NN>`, and closes with `plan: close wave …`. `main` moves to the wave head.

A story's typed scope (`aep plan artifact scope`, read by `aep plan artifact waves`) lists the
authored files it changes. It leaves out the integration files almost every story touches:
`contracts/suite.json`, `contracts/schema/`, `contracts/baseline.json`,
`contracts/ess-inputs.yaml`, `Cargo.lock`, `docs/llmgw-capability-matrix.md`, `README.md` and
this file. Whoever merges a wave owns those: it regenerates the generated ones from the merged
`spec/` and scenarios, and merges the rest by hand. A story whose purpose is one of those files
(`story:ess-0-53`) lists it. Its wave holds no other story that edits the specification, adds a
scenario or a dependency, or changes the gate.

## Releases

Versions are workspace-wide (`[workspace.package] version`) and tags are bare (`0.1.0`); every
crate is `publish = false`, so a release is a source release with no assets. There is no release
workflow. The first release is 0.1.0, the version the workspace already carries.

1. On the wave's integration branch, one commit `release: llm-gateway <version>` that sets the
   workspace version (and `Cargo.lock`) and moves README.md's status line.
2. The wave pull request, green `Gate` and `Shared source gates`, merged through the bot App.
3. Tag `<version>` on the merge commit and publish the GitHub Release `llm-gateway <version>`, both
   as the bot (`b10x-gates bot -- tag` and `push`, `b10x-gates api` `POST /releases`).
4. Verify the tag's commit, `Gate` and `Shared source gates` on it, and the Release; only then
   report it released.

## Publishing

Every commit and push is `b10x-bot[bot]`'s, through `b10x-gates bot`; verify author and committer
before pushing. Every other GitHub write (pull request, comment, issue, release, workflow dispatch,
re-run) goes through `b10x-gates api`. `gh` is authenticated as a person and is read-only here.
`~/beyond10x/AGENTS.md` has the commands.

## Never

- Write code that runs in anything but Rust; command lines use clap derive.
- Make a paid call or provision an external resource from a test.
- Depend on an llm crate by path, or copy llm's specification domains in.
- Edit `contracts/suite.json` or `contracts/schema/` by hand.
- Commit a `/home/<name>/` path: the common Gates `personal-paths` check has no exception.
- Change the repository outside a managed worktree, or write to GitHub with `gh`.
