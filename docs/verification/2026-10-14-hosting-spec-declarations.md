# Hosting specification declarations, 2026-10-14

The re-run of the `ess:hardening` design review (technique 8) for
`story:hosting-spec-declarations`, on `impl/hosting-spec-declarations` (base `c6b31e7`), after the
story declared rows 5-46 of [spec-hardening-hosting-review.md](spec-hardening-hosting-review.md).
Line numbers refer to the tree this record was written in. `ess` is 0.59.0; the system is on
`format: ess/23`, whose enum attributes carry each hosting refusal's code and message, each
transition's `from` and `to` (typed `llm-gateway.hosting.Phase`, so ESS refuses an edge naming a
phase that does not exist), and each Runpod refusal's code.

## Inputs

| Side | Files |
| --- | --- |
| design | `docs/hosting.md` (`D`) |
| mapping | none |
| specification | `spec/domains/hosting.yaml` (`H`), `spec/domains/runpod.yaml` (`R`), validated first: `llm-gateway v1 — 8 file(s), valid` |

The brief is `references/design-review.md` of the `ess:hardening` skill. The review was run by the
implementing agent as a read of both sides, not by a separate agent, so it knew the plant. The
plant shows the procedure reaches the declaration; it does not show an independent reader would.

## The plant

On a copy of `spec/` in the unit's scratch directory, the edge
`{name: RequestedDisowned, attributes: {from: Requested, to: Disowned}}` was deleted from
`llm-gateway.hosting.Transition`. `ess specify validate --path <copy>/spec` printed
`llm-gateway v1 — 8 file(s), valid`: ESS has no way to know an edge is missing. The review of the
copy returned it as finding 1.

## Findings

| # | classification | design (file:line, quoted) | spec (file:line, quoted) | note |
|---|---|---|---|---|
| 1 | missing | D:82 "Requested   -> Requested \| Active \| StopRequired \| Stopped \| Disowned" | H:61 (copy) "`{name: RequestedStopped, attributes: {from: Requested, to: Stopped}}`", the last `Requested` edge | The plant, caught. The real specification declares it at H:62. |
| 2 | contradicts | D:244 "Four of those vocabularies are re-declared by hand in `spec/domains/hosting.yaml`"; D:238-239 lists the closed vocabularies without `EvidenceKind` | H:20, :100, :110, :122, :165, :204, :221: seven vocabularies, and the transition table at H:49 | The design is stale, not the specification. The fix is a docs edit outside the sentences this story may change in `docs/hosting.md`; it is left as a patch for the wave's integration. |
| 3 | spec-only | D:51-56, the evidence table, names four routes but no code for the last two | H:211-212 "`code: provider-reference`", "`code: caller-reference`" | The codes name the table's rows; the same patch adds them to the table. |
| 4 | missing | D:297-299 "The crate links `b10x-llm-credentials` and `tokio` … and uses neither" | R:3-9, the domain header | Declared during the re-run: `ESS-LIMIT:` note at R:10, held by `the_crate_uses_nothing_of_llm_providers_but_the_runpod_description`. |
| 5 | missing | D:373 "an 8 MiB bound each"; D:374-375 "No part of either stream reaches the transport's `Debug`" | R:178 "read under the same deadline and size bound" | Declared during the re-run: `ESS-LIMIT:` note at R:180, held by `a_stream_is_read_up_to_eight_mebibytes_and_not_one_byte_more` and `a_failure_on_stderr_reaches_no_debug_output`. |
| 6 | missing | D:161-162 "`Observe` adopts a resource carrying that request id" | H:77-79, `StopReason` `AmbiguousMutation` | Declared during the re-run in the `AmbiguousMutation` entry, pinned by `an-ambiguous-create-is-resolved-by-adopting-what-it-left-behind`. |
| 7 | missing | D:175-176 "after which this controller issues no mutation against it at all" | H:199-200, `EvidenceKind` comment on `Disowned` | Declared during the re-run, pinned by `a-newer-epoch-disowns-the-record-and-we-stop-mutating-it`. |
| 8 | missing | D:500-513, `RunpodPool::invalidate`, `ensure_running`, pods outliving the process | R `Pool` | Declared in `llm-gateway.deployment` (deployment.yaml:151-155); the re-run added pointers to it in the `Pool` comment, with the `w7_*` and `d2_*` tests. |

Both directions were read: every behaviour statement in `D` was matched to a declaration in `H` or
`R`, then every declaration in `H` and `R` to a statement in `D`. Statements about how the tests
are organized (D:116-123, the grid in `machine.rs`; D:257-274, the guards table) were matched to
the declarations they justify, not counted as behaviour.

Counts on the copy: 6 missing (the plant and findings 4-8), 1 contradicts, 0 stale mapping, 1
spec-only, 0 unclear. Without the plant: findings 4-8 are declared in this tree, findings 4 and 5
as `ESS-LIMIT:` notes naming their tests. Finding 2's specification side is declared and true;
its design side waits for the patch named below.

Not reached: D "The pool the binary composes" (D:409-499) beyond the pool's own calls, which
`spec/domains/deployment.yaml` declares and this story does not change; D "Verification and
limits" and both "What this does not establish" paragraphs, which promise nothing.

## The rows of the 2026-10-06 review

Rows 5-39 are `closed`, rows 40-42 `decided` and rows 43-46 `justified` or `deleted`; the status
column of [spec-hardening-hosting-review.md](spec-hardening-hosting-review.md) names, for each, the
declaration and the scenario or test that pins it. `story:runpod-production-transport` closed none
of them whole: its `CreateAnswer` note ("A `Lost` create is never retried, here or by the
provider") is part of row 30, and its `lastStartedAt` note part of row 31.

Every scenario slug and test name the two domains cite in backticks was checked against the tree
by a script in the unit's scratch directory: 130 cited, 0 missing.


## Runs

On Rust 1.98.0, the toolchain CI uses, on the unit's final tree:

- `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo test -p <crate>` for
  `b10x-llm-provision`, `b10x-llm-runpod` and `b10x-llm-gateway-conformance`: every one exit 0,
  217 tests passed, 0 failed, after one Clippy fix (`map_or_else` in
  `checks/conformance/src/hosting.rs`).
- `cargo run --locked -p b10x-llm-gateway-conformance -- check`: exit 0,
  `{"total":202,"passed":202,"failed":0,"error":0,"unsupported":0,"skipped":0}` on each of three
  runs.
- Acceptance 5: with `(Phase::Requested, Phase::Disowned)` deleted from `TRANSITIONS` in
  `crates/llm-provision/src/machine.rs`, the same check exits 1 with
  `{"total":202,"passed":200,"failed":2,...}`; the two failures are
  `a-create-taken-over-before-any-listing-confirmed-it-is-disowned` and
  `the-ownership-state-machine-is-exactly-this-table`. The file was restored afterwards.

The new cases were not run red on their own before the declarations and the `EvidenceKind` enum
existed: the unit was written while no build could run, so the cases and the code they test
landed together.
