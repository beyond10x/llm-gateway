---
format: aep.planning-md/3
id: review-result:adversary-w02-a-call-starts-no-pod-pass-1
kind: review-result
status: active
title: 'Adversary, pass 1 (story:a-call-starts-no-pod, wave 2026-11-25-w02): green, 0 red cases, 5 cases added, 2 mutants the suite missed, 8 findings'
relations:
- reviews: story:a-call-starts-no-pod
revision: 1
---
The adversary's report of pass 1 on `impl/a-call-starts-no-pod` at 3dd9e5d7, as it returned.
Paths of the coordinator's scratch directory are written `<scratch>/`.

```
unit: story:a-call-starts-no-pod, findings cover branch impl/a-call-starts-no-pod at 3dd9e5d7 plus my uncommitted test hunk
verdict: green
cases: executed 544→549, red 0
origin: introduced 8, pre-existing 0, undecided 0
wrote-outside-worktree: 3 scratch clones and 11 files, all under <scratch>/ (listed in part 6)
needs-coordinator: yes. Decide whether my 5 uncommitted cases join the unit: they kill 2 mutants the suite misses. Route findings 2 and 3 to the implementor.
```

**Summary:** I found no request path that creates a pod without the key. The only pod creates
happen in `RelayTargets::acquire` (`relay.rs:1905`, then `relaying.rs:584`) and in the wake.
Without the key, `acquire` calls `ensure_held_running`, which reaches `hand_out(alias, None, …)`,
and with `None` the pool cannot create. All 5 cases I added pass on the head. Two of them kill
mutants that the unit's 544 tests miss, and one existing test can no longer fail since this
change. There is also doc drift.

## 1. git --no-pager diff --stat

```
 crates/llm-gateway-cli/tests/relaying.rs | 241 +++++++++++++++++++++++++++++++
 1 file changed, 241 insertions(+)
```

The only change is one hunk appended at line 1717 of a test file. I added no new test binary and
touched no other file.

## 2. Cases added (`crates/llm-gateway-cli/tests/relaying.rs`, section "adversary, story:a-call-starts-no-pod")

All 5 are green now. I ran them alone first (`cargo test --locked -p b10x-llm-gateway-cli --test
relaying adv_w02`, on the build node): `test result: ok. 5 passed; 0 failed; … 33 filtered out;
finished in 0.52s`. I reran them after `rustfmt`, with the same result.

- `adv_w02_a_daily_cap_without_the_key_lets_no_call_start_a_pod_and_the_wake_still_does`: the
  provider has a `daily_cap_usd` but the model has no key. The call answers `model-stopped` at
  once, with 0 creates and 0 spend reads. The owner's wake then answers 202 with 1 create and 1
  spend read. Holds Acceptance 1 (default `false`), 2 and 4.
- `adv_w02_a_second_model_that_sets_the_key_starts_only_its_own_pod`: two models on one provider;
  `big` has the key, `small` does not. Calls to `small` before and after are `model-stopped`;
  `big` is relayed; exactly one create, and it is `big`'s. Holds Acceptance 2 and 5.
- `adv_w02_a_call_after_the_woken_pod_failed_a_relay_starts_no_replacement`: wake, then
  `upstream-failed` (which invalidates the pod), then the next two calls are `model-stopped`.
  Creates stay at 1 and there is 1 termination, with the terminate accepted. Holds Acceptance 3
  and the Decided line "a pod that fails … starts no replacement".
- `adv_w02_the_shipped_composition_without_the_key_sends_no_create_on_a_call_and_one_on_a_wake`:
  runs through `start_connected` with the `connectors` fixture. A call sends no `invoke
  pod.create` and no `approvals prepare pod.create`, and answers `model-stopped`; the wake sends
  exactly one create. Holds Acceptance 2 in the composition the binary actually ships; no
  existing test covered that without the key.
- `adv_w02_another_providers_ceiling_does_not_bind_the_owners_wake_on_the_connected_account`:
  replaces the coverage lost in finding 2. It drives the contract through the wake: 202, one
  create, no `billing.pods`. Holds `docs/hosting.md` "Spend ceilings": "A ceiling declared on
  another provider does not bind the pool … the pool reads no spend".

## 3. Suite run (after the cases existed)

Command, in the worktree on the build node: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test
--locked -p b10x-llm-gateway-cli`. Exit 0, 65 test binaries, `549 passed; 0 failed; 0 filtered`.
The slowest binary finished in 12.01 s and none of my cases takes over 1 s.

The before count, 544, comes from the same crate run in the mutant-1 copy with `-- --skip
adv_w02`. That run filtered exactly 5 tests (mine) and passed 544.

**Mutants** (applied only to scratch clones; the worktree was never edited):

- **Mutant 1** (`mutant1.diff`) at `crates/llm-gateway-cli/src/relaying.rs:387`: the provider's
  `daily_cap_micro_usd` also counts as `start_on_request`.
  - The whole crate suite without my cases **survived**: 544 passed, 0 failed.
  - My cases **killed** it, 2 of 5 red. `adv_w02_a_daily_cap…` failed at line 1768 with `502 …
    "upstream-failed"`: the call started a pod. `adv_w02_a_second_model…` failed at line 1822
    with `200 OK {"id":"big"}` on `small`'s call.
  - By inspection the conformance suite misses it too: its only capped scenario sets the key.
- **Mutant 2** (`mutant2.diff`): drop the `billed` filter in `spend_policy` (line 214) and
  `subset.providers.retain` in `start_connected` (line 1028). This is the defect
  `adversary_w12_spend_pass2.rs` was written against.
  - On the head, `adversary_w12p2_another_providers_ceiling_does_not_bind_the_connected_account`
    **survived** (ok). My replacement case **killed** it: line 1955, `503 … "spend-ceiling"` on
    the wake.
  - On the base e6478aeb with the same mutant, that existing test was **red**:
    `adversary_w12_spend_pass2.rs:89: … spend was read 1 time(s) against another provider's 1 USD
    ceiling`.

## 4. Findings

1. **The suite misses mutant 1.** `crates/llm-gateway-cli/src/relaying.rs:387`.
   - What was measured: mutant 1 survived 544 tests and is killed by the two cases above.
   - What reaches it: the configuration the operator is most likely to set, a daily cap for wakes
     without the key.
   - CONFIRMED, introduced. Fix: keep the two cases.
2. **A test that can no longer fail.** `crates/llm-gateway-cli/tests/adversary_w12_spend_pass2.rs:55`.
   - Its call now answers `model-stopped` and never reaches a start, so `reads.is_empty() &&
     !answer.contains("spend-ceiling")` holds whatever the composition does. The same mutant
     turns it red on the base and leaves it green on the head.
   - It cannot be fixed by adding the key: the key needs a daily cap on the connected provider,
     and the document allows a cap on only one provider, so the load is refused.
   - NEEDS-CHANGE, introduced. Fix: drive it through the wake, as my replacement case does.
3. **Contract drift in `docs/hosting.md:743`**: "so the next request starts a replacement" (the
   W7 paragraph). This is false for the new default; the third case measures the opposite. The
   `invalidate` doc comment at `crates/llm-gateway-cli/src/relaying.rs:657-659` ("the next
   acquisition starts a replacement") has the same drift. NEEDS-CHANGE, introduced.
4. **Contract drift in the `upstream-failed` client message**,
   `crates/llm-gateway/src/error.rs:175` and `docs/gateway.md:1302`: "the next request asks for a
   replacement". For a model without the key, the next request is `model-stopped`. CONFIRMED,
   introduced, note.
5. **Incomplete restore instruction in `CHANGELOG.md:12`**: "set `start_on_request = true` … and
   `daily_cap_usd` on its provider". On a pre-change document this is refused `config:value`,
   because declaring a cap also requires `price_per_hour_usd` on every model
   (`config.rs:1186-1190`). `README.md:211-213` has the same gap; `docs/model-profiles.md` gets
   it right. Acceptance 7 only requires the key to be named, and the brief's list does not
   include `CHANGELOG.md`, so this is a judgement finding with no case. CONFIRMED, introduced.
6. **Acceptance 2 during the gateway's graceful stop**,
   `crates/llm-gateway-cli/src/relaying.rs:590-591`. Once `stopping` is set, a call to a stopped
   model without the key answers `target-unavailable`, not `model-stopped`. The implementor chose
   this on purpose. It is reachable only inside the stop window, and I could not write a
   deterministic case. INFEASIBLE, introduced, note.
7. **Misplaced doc comment**, `crates/llm-gateway-cli/src/config.rs:1269-1276`. The comment for
   `providers()` ("`[providers.<name>]`, checked: …") now sits on top of
   `start_on_request_capped`, and `providers()` at line 1295 has none. CONFIRMED, introduced,
   note.
8. **The cap check can be bypassed by a library caller**,
   `crates/llm-gateway-cli/src/config.rs:931` and `crates/llm-gateway-cli/src/relaying.rs:917`.
   `Model::start_on_request` is a public field, and `pod_targets` / `start_relaying` compose from
   any `Deployment`, so a library caller can set `true` with no cap. The binary only goes through
   `parse`, and I found nothing that reaches this. INFEASIBLE, introduced, note.

## 5. Attacked and could not break

- Each wire (chat, responses, messages), the clamp's count, a tool request, and a call to a
  `stopping` pod: no create without the key.
- A pod that fails while held: no replacement.
- `request_hold_seconds = 0`.
- Loader: cap on another provider, total cap only, a daily cap of 0 or negative, two providers, a
  provider reached through `connectors`, a document that sets nothing, and the `model-profiles`
  blocks.
- `model-stopped`: 503, no `retry-after`, outcome `refused`, the message, and its rows in the
  refusal table, the outcome table, the generated reference and the suite.
- The pool's `hand_out` for every phase compared with `status_of`: no case where status says
  `stopped` while a call is held.
- The startup sweep and `reap` create nothing.

## 6. Paths written outside the worktree

All under `<scratch>/`:

- Clones with build output: `mut/` (2.2 GB, mutant 1), `mut2/` (802 MB, mutant 2 on the head),
  `mut3/` (745 MB, mutant 2 on the base). They are standalone clones made with `git clone`, not
  worktrees. Their cargo runs went through the shim and printed no node sync line.
- Scripts: `make-copy.sh`, `make-copy2.sh`, `make-copy3.sh`
- Mutant diffs: `mutant1.diff`, `mutant2.diff`
- Logs: `run1-cases-alone.log`, `run2-cases-formatted.log`, `mutant1-suite.log`,
  `mutant1-cases.log`, `mutant2-cases.log`, `mutant2-base.log`, `suite-after.log`, `fmt.log`

## 7.

```findings
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 387
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a mutant that lets a provider's daily_cap_usd act as start_on_request survives all 544 crate tests; only the adversary's cap-without-key and two-model cases kill it
- file: crates/llm-gateway-cli/tests/adversary_w12_spend_pass2.rs
  line: 55
  category: mutant
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: since calls start no pod this test passes whatever the composition does with another provider's ceiling (red on the base under the mutant, green on the head); it must drive the wake instead
- file: docs/hosting.md
  line: 743
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the W7 paragraph still says the next request after a failed relay starts a replacement, which is false for the new default (also the invalidate doc comment at relaying.rs:657)
- file: crates/llm-gateway/src/error.rs
  line: 175
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the upstream-failed client message says the next request asks for a replacement, but without start_on_request the next request is model-stopped
- file: CHANGELOG.md
  line: 12
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the restore instruction names the key and daily_cap_usd but not the price_per_hour_usd every model then needs, so following it on a pre-change document is refused config:value
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 590
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: during the gateway's graceful stop a call to a stopped model without the key is target-unavailable rather than the model-stopped Acceptance 2 names; reachable only in the stop window
- file: crates/llm-gateway-cli/src/config.rs
  line: 1269
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the doc comment of providers() now heads start_on_request_capped, leaving providers() undocumented
- file: crates/llm-gateway-cli/src/config.rs
  line: 931
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: the public Model::start_on_request field plus the public pod_targets and start_relaying seams let a library caller enable request starts without the loader's cap check; the binary never does
```
