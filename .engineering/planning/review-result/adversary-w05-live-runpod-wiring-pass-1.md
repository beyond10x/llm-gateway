---
format: aep.planning-md/3
id: review-result:adversary-w05-live-runpod-wiring-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w05 adversary, story:live-runpod-wiring, pass 1
relations:
- reviews: story:live-runpod-wiring
revision: 1
---
```
unit: story:live-runpod-wiring, impl/live-runpod-wiring at ab37a42 (unit head c8d03ba plus one adversary test commit)
verdict: NEEDS-CHANGE
cases: executed 129→133, red 2
origin: introduced 3 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 path (~/.cache/llm-gateway-w05/wiring/adversary/)
needs-coordinator: yes (story Acceptance 1 and 3 still say "binary" / "process test"; the shipped binary now refuses those documents)
```

The binding rule holds: I found no way for the shipped binary to create a billed pod. Two library-level cases are red: in `start_connected`, whether a document is accepted depends on whether the connectors connection is up.

**1. Diff stat** (every path is a test file)
```
 crates/llm-gateway-cli/tests/adversary_w05_wiring.rs       | 171 +++
 crates/llm-gateway-cli/tests/adversary_w05_wiring_event.rs |  78 +++
 2 files changed, 249 insertions(+)
```
Commit `ab37a42` `test: a document the pool refuses is refused before any connectors call (red)`. Both files were untracked; I staged them and committed them through the bot.

**2. Cases** (red output from running the new files alone, before the suite)

| Case | Asserts | Now |
|---|---|---|
| `adversary_w05_a_document_the_pool_refuses_is_refused_before_any_connectors_call` | Connection up, a model with `image = "vllm/vllm-openai v0.27.1"` (contains a space). `load` accepts it. `start_connected` must refuse `config:value` with no connectors calls. | red |
| `adversary_w05_a_document_the_pool_refuses_is_refused_with_the_connection_down` | The same document with the connection down must also be refused `config:value`. | red |
| `adversary_w05_every_spelling_of_connectors_is_refused_before_any_call` | 8 spellings of the table (plain, inline, dotted keys, quoted key, `\u0065` escape, `[[array]]`, `Connectors`, table on a second provider with no models). Each is refused by `load` (schema/value) or by `start` (`config:value`), with no connectors calls. | green |
| `adversary_w05_the_unreachable_event_is_written_and_carries_no_secret` | When unreachable: exactly 1 `WARN` line with `provider="runpod"`. When reachable: 0 lines. At trace level, no vLLM key and no owner secret in either. | green |

Verbatim red output:
```
panicked at crates/llm-gateway-cli/tests/adversary_w05_wiring.rs:145:5:
the document was refused only after the fixture received [Object {"argv": [... "operations","describe",... "pods.list"]}, Object {... "key": "operations invoke pods.list"}]
panicked at crates/llm-gateway-cli/tests/adversary_w05_wiring.rs:163:13:
a model the pool cannot run was accepted because the connection was down; the fixture received [... "operations describe pods.list" ..., ... "operations invoke pods.list"]
test result: FAILED. 1 passed; 2 failed
```
The event case was red once in the shared file. That was my test's fault: another test reached the same `tracing` callsite on a thread with no subscriber. Run alone it passed, so I moved it into its own test binary. I am not reporting it as a finding.

**3. Suite** (`RUSTUP_TOOLCHAIN=1.98.0 CARGO_INCREMENTAL=0 cargo test -p b10x-llm-gateway-cli --locked --no-fail-fast`; free disk 25G, load 3.83)
```
tests/adversary_w05_wiring.rs: test result: FAILED. 1 passed; 2 failed
tests/adversary_w05_wiring_event.rs: ok. 1 passed
binary.rs ok 36 · config.rs ok 21 · live_runpod.rs ok 5 · relaying.rs ok 19 · every other binary ok
EXIT=101
```
133 cases ran. The before figure (129) is 133 minus my 4 cases, taken from the per-binary counts of this same run; I did not run the suite separately without them. Clippy `-D warnings` and `cargo fmt --check` are clean.

**4. Findings**

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| F1 | `crates/llm-gateway-cli/src/relaying.rs:706` | NEEDS-CHANGE / introduced | `runpod_pool(&subset…)`, which runs the model checks, comes after `reachable()` at `:692`. Connection up: refused only after `operations describe` + `invoke pods.list`. Connection down: the same document starts. This contradicts the comment at `:682` ("whether a document is valid never depends on it"). | Library only today: `start_connected`. `start` refuses connectors documents first (`serve.rs:181`). It becomes live in the binary when story:pod-proxy-tls removes that guard: a deployment would start during a connectors outage and then be refused on the next restart. Suggested fix: build `runpod_models(&subset)` / `hosting_policy` and the `PoolTargets` checks before `ConnectorsRunpod::new`. |
| F2 | `crates/llm-gateway-cli/src/relaying.rs:658` | CONFIRMED / introduced | The doc says "No transport but `ConnectorsRunpod` can be composed here". In fact `wrap: FnOnce(ConnectorsRunpod) -> W` can ignore its argument and return `EmulatedRunpod`. | Callers of `start_connected`: only `serve.rs:173` (identity) and the tests. The shipped binary is not affected; the doc line is wrong. |
| F3 | `crates/llm-gateway-cli/tests/live_runpod.rs:229` | CONFIRMED / pre-existing | Acceptance 1 says "one CreatePod with the declared GPU types". The test asserts `["NVIDIA L40S"]` out of the two declared types. `connectors.rs:640` sends one `gpuTypeIds` entry per create (one GPU type per create attempt). | Transport design at the unit base 542d29f. The difference is in the acceptance wording only. |
| F4 | story `story:live-runpod-wiring` Acceptance 1 and 3 | CONFIRMED / introduced | Both still say the binary starts and serves a connectors document. Since c8d03ba it refuses one with `config:value`. Acceptance 1 is now proved at library level only. | The wave's binding rule. The coordinator owns the story text. |

**5. Attacked, could not break**
- `main.rs`: clap has only `--config`. No environment variable is read except `RUST_LOG`. No reload, and the signal handling only stops the process. `start` is the only call, and `inert_until_tls` runs first.
- The guard (`serve.rs:181`) and `start_connected` use the same check (`provider.connectors.is_some()`), so no document can get past one and not the other. All 8 spellings are covered by my green case.
- The test-only seams (`start_relaying`, `wrap`, `pods`) are reachable from no document and no flag. The `gateway-connectors-fixture` `[[bin]]` is built with the package but runs only when a test points at it.
- The "more than one connectors provider is config:value" rule: `config.rs` ensure `<= 1`, pinned by `connectors_at_most_one_provider_declares_a_connection`.
- An unreachable connection (missing executable, `0750` work directory, failed listing) makes no further call. The existing live tests cover it.
- 503 instead of 405 for unconnected models: an unknown alias in `PoolTargets::acquire` is `Unavailable`. No conformance scenario describes the shipped binary's 405; the `suite.json` 405 rows are wire-path method checks.
- The `warn` event is now observed by a case. The only `tracing` events in the sources (`main.rs:34`, `relaying.rs:694`, `logging.rs:57`, plus the pod events) carry no key.

**6. Paths outside the worktree**
- `~/.cache/llm-gateway-w05/wiring/adversary/` holds `red-alone.log`, `red-alone-2.log` and `suite.log`. I also created and deleted an empty `fix_header.py` there by mistake.

**7.**
```findings
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 706
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: start_connected runs the pool's model checks only after the reachability pods.list, so a document the pool refuses costs a connectors call when the connection is up and is served when it is down, contradicting the comment at :682
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 658
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the doc claims no transport but ConnectorsRunpod can be composed, yet wrap may discard its argument and return any RunpodTransport
- file: crates/llm-gateway-cli/tests/live_runpod.rs
  line: 229
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: Acceptance 1 says the create carries the declared GPU types while the transport sends only the first candidate per create and the test asserts that
- file: story:live-runpod-wiring
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Acceptance 1 and 3 still describe the binary serving a connectors document, which the inert-until-TLS refusal now makes impossible
```
