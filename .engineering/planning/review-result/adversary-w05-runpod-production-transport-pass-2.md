---
format: aep.planning-md/3
id: review-result:adversary-w05-runpod-production-transport-pass-2
kind: review-result
status: active
title: Wave 2026-10-08-w05 adversary, story:runpod-production-transport, pass 2
relations:
- reviews: story:runpod-production-transport
revision: 1
---
```
unit: story:runpod-production-transport, tree lg-w05-transport, findings cover e0b19b1 (cases on top in 6986639, rustfmt-only 4e9d232)
verdict: red
cases: executed 95→99, red 4
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 17 paths, all under ~/.cache/llm-gateway-w05/transport/adversary2/
needs-coordinator: yes. Findings A and B are blockers to route back to the implementor; the fixture has to change together with the code.
```

**Verdict: the transport cannot read anything the real connectors v0.36.0 CLI prints.** The fixture invented an output shape, and the code and its 22 tests agree with that shape. So with the real CLI nothing is ever sent, and the F7 correction never runs. No double-billing path exists. Against the fixture's shape, the corrections for F1, F2, F3, F4, F6 and F7 hold.

**1. Diff stat** (`e0b19b1..HEAD`), test paths only:
```
 .../tests/adversary_w05_transport_pass2.rs         | 215 +++++++++++++++++++++
 crates/llm-runpod/tests/fixture/connectors.rs      |   5 +
```
The fixture change only adds things: an answer can now carry a `stderr` key.

**2. Cases** in `crates/llm-runpod/tests/adversary_w05_transport_pass2.rs`. Each was run alone first (`--test adversary_w05_transport_pass2`). Line numbers below are after the rustfmt commit, which moved them by up to 4; the quoted messages come from the first run.

| Line | Asserts | Red output |
|---|---|---|
| :119 | an enveloped `operations describe` and `pods.list` are read | `left: 0 right: 1` (pods.list was never invoked) |
| :151 | an enveloped `applied` create is `Created` | `a pod Runpod created and connectors classified 'applied' is Lost` |
| :171 | a `refused` create reported on stderr is `Refused` | `left: Lost right: Refused` |
| :188 | a `not_granted`/`admission` read reported on stderr is revalidated once | `left: 0 right: 1` |

**3. Suite run** after the cases existed: `RUSTUP_TOOLCHAIN=1.98.0 CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8 cargo test -p b10x-llm-runpod --locked --no-fail-fast`
```
ok 4 / ok 6 / ok 3 / ok 1 / ok 3 / ok 6 (adversary_w05_transport) / FAILED. 0 passed; 4 failed (adversary_w05_transport_pass2) / ok 2 / ok 48 / ok 22 / ok 0
EXIT=101
```
- **Count:** 95 is this same run with my 4 cases left out.
- **Other gates:** fmt check and clippy `-D warnings` are clean.
- **Pass 1's cases:** all 6 are green now.

**4. Findings** (covering e0b19b1)

| # | Where | Verdict / origin | What reaches it |
|---|---|---|---|
| A | `transport/connectors.rs:326` (also :141, :151, :291, :469) | NEEDS-CHANGE / introduced | **Success answers are read one level too high.** connectors' contract (`contracts/cli/v1alpha1/semantics.md:178-180`) wraps every success as `{"ok":true,"result":…}`, and its own tests read it that way (`json_answers.rs:38`, then `result["schema"]`). The transport reads `schema`, `mutation`, `result.body`, the approval's subject digest and the connection revision from the top level. So `describe()` returns `None` and every call through the real CLI ends Lost or incomplete before anything is sent: acceptance 1 fails against the published interface. `tests/fixture/connectors.rs` prints answers without the envelope and has to change with the code. Reached once anything builds a `ConnectorsBinding`; nothing does yet (pass-1 F4). |
| B | `transport/connectors.rs:204` | NEEDS-CHANGE / introduced | **Every failure answer is thrown away.** Failures go to stderr with stdout empty (same contract, and `local_cli.rs:207,474`), and the transport sets `.stderr(Stdio::null())`. Three consequences: a `refused` create is Lost, so the next GPU is never tried; an `unknown` create skips its one listing; `refused_at_admission` never matches, so **the F7 correction cannot fire against the real CLI**. |
| C | `transport/connectors.rs:310` | INFEASIBLE / introduced | Judgement, not tested. `(schema, revision)` is cached for the life of the process. connectors' upgrade path changes the descriptor revision (`docs/local-catalog-provider.md:396-399`), and from then on every call carries a stale revision until restart. That path also answers `lifecycle_conflict`, which the transport does not revalidate on. |

**5. Attacked, could not break**
- **F1, process cleanup:** every path reaps the CLI.
- **F1, the owner:** connectors starts its owner with `setsid` and stdio pointed at `/dev/null` (`owner/transport.rs:783-805` at v0.36.0). So the group kill never reaches the owner, and the owner never holds the CLI's stdout.
- **F1, the deadline:** it applies per CLI call, as the spec says. A create can chain about 13 bounded calls under the pool mutex.
- **F2:** request ids are unique per create (`pool.rs:313`, which includes a nonce). Two pods with the same tag, an incomplete listing, or a missing tag all give `Lost`.
- **F7, no second send:** the repeat follows only a non-zero exit at stage `admission` with no classification or `not_attempted`. A revalidate that fails, hits a revision conflict or hangs returns the original answer, so the result is `Lost`. The new idempotency key bypasses nothing, because nothing was dispatched.
- **F3, F4, F6** hold.
- **Terminate:** `refused` → `Rejected`, and the controller still owes the stop (`controller.rs:643-651`).

**6. Written outside the worktree**, under `~/.cache/llm-gateway-w05/transport/adversary2/`:
- `red-cases.log` and `suite.log`.
- `upstream/`, 15 files read from connectors v0.36.0 through `gh api`: `semantics.md`, `json_answers.rs`, `local_cli.rs`, `owner_replay.rs`, and 11 files named `apps_connectors_src_*` / `crates_connectors-*`.

The tests also wrote `target/tmp/adversary-w05-transport-pass2/` inside the tree.

**7. Findings block**
```findings
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 326
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: connectors wraps every success in {"ok":true,"result":...}, and the transport and its fixture read schema, mutation, result.body, the approval subject and the connection revision from the top level, so every real call ends Lost or incomplete before anything is sent
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 204
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: connectors writes every failure to stderr with stdout empty, and the transport discards stderr, so refused creates are Lost and the not_granted revalidation (F7) never fires
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 310
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the operation schema and revision are cached for the process lifetime, so after a connectors configuration upgrade every call carries a stale revision until restart
```
