---
format: aep.planning-md/3
id: review-result:adversary-w05-runpod-production-transport-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w05 adversary, story:runpod-production-transport, pass 1
relations:
- reviews: story:runpod-production-transport
revision: 1
---
unit: story:runpod-production-transport, tree lg-w05-transport, findings cover `3b09066` (cases on top in `a268896`)
verdict: red
cases: executed 85→91, red 5
origin: introduced 7 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 paths, all under ~/.cache/llm-gateway-w05/transport/adversary/
needs-coordinator: yes — the blocker in F2 needs a fix routed to the implementor

**1. Diff stat** (`3b09066..HEAD`). Test paths only, so the bound held:
```
 crates/llm-runpod/tests/adversary_w05_transport.rs | 318 +++++++++++++++++++++
 crates/llm-runpod/tests/fixture/connectors.rs      |  19 ++
```
The fixture change only adds things: a `hold_stdout_ms` answer key and a `__hold_stdout` child mode. `tests/transport.rs` still shows 18 passed.

**2. Cases added**, all in `crates/llm-runpod/tests/adversary_w05_transport.rs`. Each was run alone first (`--test adversary_w05_transport`):

| Line | Asserts | Now |
|---|---|---|
| :147 | a list with a 1.5 s timeout returns in under 5 s when a process the CLI started keeps its stdout open | red: `one list with a 1.5 s per-call timeout took 8.020617929s` |
| :170 | an `unknown` create whose only same-named pod carries `B10X_LLM_REQUEST=req-0` is `Lost` | red: `left: Created(Pod { id: "old999", … "B10X_LLM_REQUEST": "req-0" …}) right: Lost` |
| :191 | the same case through `RunpodProvider::create` gives `Dispatch::Unknown` | red: `accepted Some(Identifier("old999")) left: Accepted right: Unknown` |
| :233 | the create body for a pod without a cache sends `"volumeInGb": 0`, as llmgw does | red: `left: Null right: Number(0)` |
| :248 | terminate maps `refused`/`not_found` → `Refused`; `unknown`, `not_attempted`, timeout and unparseable output → `Lost`; one send each | green |
| :293 | a relative `work_directory` still gives an absolute `--proof-output` | red: `"--proof-output","../../target/tmp/…proof" … "proof_output_refused":true` |

**3. Suite run** after the cases existed:

`RUSTUP_TOOLCHAIN=1.98.0 CARGO_INCREMENTAL=0 cargo test -p b10x-llm-runpod --locked --no-fail-fast`
```
ok 4 / ok 6 / ok 3 / ok 1 / ok 3 / FAILED. 1 passed; 5 failed (adversary_w05_transport) / ok 2 / ok 48 / ok 18 / ok 0
EXIT=101
```
- **Count:** the 85 "before" comes from the implementor's `gate2.log`, not from a run of mine.
- **Gates:** fmt and clippy `-D warnings` are clean on the package.
- **Tree check:** `-- --list` shows the six new tests exist in this tree.

**4. Findings**

| # | Where | Verdict / origin | What reaches it |
|---|---|---|---|
| F1 | `transport/connectors.rs:227` | INFEASIBLE / introduced | `reader.join()` has no deadline. When the CLI exits, the loop stops checking the timeout, so any process the CLI started that still holds stdout blocks the transport, under the pool mutex. connectors documents automatic owner startup, but I could not show that this owner keeps the CLI's stdout. Fix: give the join a deadline, or kill the process group. |
| F2 | `transport/connectors.rs:563-570` | NEEDS-CHANGE / introduced | Pod names are per alias, not per create (`pool.rs:984` sets `resource_name = alias`; Runpod's `Pod.name` "does not need to be unique"). An `unknown` create next to a leftover pod of the same alias returns `Created(old pod)`, and the controller then adopts it. The leftover is an exited pod, or one whose terminate came back `Refused` or `Lost`. Fix: also match `env[B10X_LLM_REQUEST]` against the request's tag. |
| F3 | `transport/connectors.rs:452` | CONFIRMED / introduced | Every create without a network volume (`cache: None`) is missing `volumeInGb`. Runpod's pinned `PodCreateInput.volumeInGb` defaults to 20 and llmgw sends 0, so each such pod gets a 20 GB pod volume. Neither document gives a price for it, so I don't know the cost. |
| F4 | `transport/connectors.rs:323` | INFEASIBLE / introduced | Nothing outside the tests builds a `ConnectorsBinding`. The spec types `work_directory` as a plain `String`, while connectors requires an absolute proof path. |
| F5 | `tests/transport.rs:351` | CONFIRMED / introduced | The only terminate case is the `applied` one, so a mutant mapping `refused` to `Terminated` would pass the existing suite. The :248 case now pins the mapping. I did not build a mutated copy to prove it. |
| F6 | `transport/connectors.rs:480` | CONFIRMED / introduced | The code comment, `docs/hosting.md:343` and the spec say `dockerStartCmd: []` clears the image CMD. The pinned OpenAPI says the opposite: "If [], uses the start CMD defined in the image". The parity with llmgw holds; the stated reason is false. |
| F7 | `transport/connectors.rs:11` | NEEDS-CHANGE / introduced | Judgement, untested against the real CLI. connectors' connection proof expires (60 s default, 300 s at most), and invokes are then refused `not_granted` until `connections revalidate`. The transport never revalidates, and neither the docs nor the spec mention it. After expiry, lists come back incomplete and creates `Lost`. |

**5. Attacked, could not break**
- **Argv injection:** pod names, model names and env values go into the input file, never argv; the CLI runs without a shell.
- **Files and proofs:** input files are created `create_new` with mode 0600 under unique stems, and the input and proof files are removed after each write. A proof is issued per write and never reused.
- **Output handling:** partial or unparseable output and output over 8 MiB all map to `Lost` or incomplete.
- **Classification:** `refused` is the only answer that gives `Refused` on create; get matches exactly one pod by `id`.
- **Keys and timestamps:** `Debug`, the probe's bearer handling and header-injection guard, and `lastStartedAt` moving forward all held.
- **JSON shapes:** prepare and issue match connectors `ess/cli.yaml`. I could not check the top-level shape of the error output.
- **Proof expiry (300 s):** only a configured timeout over 100 s could reach it, and the answer would then be `Lost`.

**6. Written outside the worktree**, all under `~/.cache/llm-gateway-w05/transport/adversary/`: `llmgw-runpod.rs`, `red-cases.log`, `suite.log`, `suite-nff.log`, `suite-final.log`. The tests also wrote under the tree's own `target/tmp/adversary-w05-transport/`.

**7. Findings block**
```findings
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 227
  category: boundary
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: the stdout reader is joined with no deadline, so a process holding the CLI's stdout keeps the transport past its per-call timeout (8.0 s against 1.5 s)
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 563
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an unknown create is resolved by a pod name that is per alias, not per create, and answers Created with an earlier request's pod, which the provider accepts
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 452
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the create body omits volumeInGb where llmgw sends 0, so Runpod's 20 GB default pod volume is rented for every pod without a cache
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 323
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a relative work_directory yields a relative --proof-output, which approvals issue refuses; no caller builds a binding yet
- file: crates/llm-runpod/tests/transport.rs
  line: 351
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: no case pinned terminate's refused/not_found to Refused or unknown to Lost, so a refused-to-Terminated mutant passed the suite
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 480
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: code, hosting.md and the spec say dockerStartCmd [] clears the image CMD, while the pinned OpenAPI says [] uses the image's CMD
- file: crates/llm-runpod/src/transport/connectors.rs
  line: 11
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the transport never runs connections revalidate, so once connectors' connection proof expires (60-300 s) every invoke is refused and the pool sees incomplete lists and Lost creates
```
