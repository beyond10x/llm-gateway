---
format: aep.planning-md/3
id: review-result:adversary-w02-a-call-starts-no-pod-pass-2
kind: review-result
status: active
title: 'Adversary, pass 2 (story:a-call-starts-no-pod, wave 2026-11-25-w02): red, 2 introduced red cases (1 warning, 1 note), 1 green case added, 6 mutants of which 1 survived the unit''s tests'
relations:
- reviews: story:a-call-starts-no-pod
revision: 1
---
The adversary's report of pass 2 on `impl/a-call-starts-no-pod` at 28a81acb, as it returned.
Paths of the coordinator's scratch directory are written `<scratch>/`.

```
unit: story:a-call-starts-no-pod
verdict: red
cases: executed 550→553, red 2
origin: introduced 5, pre-existing 0, undecided 0
wrote-outside-worktree: 19 files under <scratch>/ (listed in part 6); the mutant clone is deleted
needs-coordinator: yes. Finding 1 comes from the wording the correction decided ("every model of that provider"), so the restore instruction needs a new decision. Decide whether my 3 uncommitted cases join the unit: the green one kills a mutant that survives the unit's 550 tests.
```

Findings cover branch `impl/a-call-starts-no-pod` at 28a81acb plus my uncommitted test hunks.

**Summary:** I found no path on which a call without the owner's wake creates a pod in the
binary. Two cases are red. First, the restore instruction in `CHANGELOG.md` and `README.md`
(pass-1 finding 5) is still incomplete: following it on a document with a second provider is
refused at load. Second, through the library seam a call that may start a pod is checked against
another provider's ceiling instead of its own provider's daily cap. One relay mutant survives the
unit's own 550 tests.

## 1. git --no-pager diff --stat

```
 crates/llm-gateway-cli/tests/config.rs   | 34 +++++++++++
 crates/llm-gateway-cli/tests/relaying.rs | 99 ++++++++++++++++++++++++++++++++
 2 files changed, 133 insertions(+)
```

Both paths are test files. No implementation file was touched. `cargo fmt --all -- --check` is
clean. Clippy (`-p b10x-llm-gateway-cli --all-targets --all-features -- -D warnings`) exits 0.

## 2. Cases added, run alone first (`--test relaying adv2_w02`, then `--test config adv2_w02`, on the build node)

**Red 1.** `crates/llm-gateway-cli/tests/config.rs:2065`
`adv2_w02_the_restore_instruction_loads_a_pre_change_document_with_a_second_provider`. It is red
now.

- The case takes a document that loads before the change: `runpod` with `small`, plus a second
  provider `reserve` with a model `spare`. It adds exactly what the instruction names (the key on
  `small`, `daily_cap_usd` on `runpod`, a price on every model of `runpod`), and asserts that the
  document loads.
- Runner output:

  ```
  panicked at crates/llm-gateway-cli/tests/config.rs:2084:25:
  the restore instruction of CHANGELOG.md and README.md was followed and the document is refused: config:value: models.spare.price_per_hour_usd is required once a provider declares a spend ceiling
  ```

- Sentence held: correction decision 5, "the instruction that restores the old behaviour names
  everything a document needs". The loader (`config.rs:1186-1190`) wants a price on every model
  of every provider once any provider declares a ceiling. `docs/model-profiles.md:25` says "every
  model's"; `CHANGELOG.md:12` and `README.md:213` say "every model of that provider". The decided
  wording is itself the cause.
- What reaches it: an owner with a second provider whose models answer `target-unavailable`, a
  setup `README.md` describes. Severity is small: the start is refused with a message that names
  the missing key; no money is at risk.
- Origin: introduced.

**Red 2.** `crates/llm-gateway-cli/tests/relaying.rs:2033`
`adv2_w02_a_call_that_may_start_a_pod_is_checked_against_its_own_providers_daily_cap`. It is red
now.

- The case builds a `Deployment` from the loaded document: `small` has the key under `runpod`'s
  10 USD daily cap. It adds a provider `other`, which sorts before `runpod`, with a total cap
  only and a model `spare` of its own. The account has spent 50 USD today. It composes through
  `pod_targets` and asks for `small`.
- Runner output:

  ```
  panicked at crates/llm-gateway-cli/tests/relaying.rs:2050:5:
  assertion `left == right` failed: a call started a pod with 50 USD spent today under its provider's 10 USD daily cap (Some(ColdStart))
    left: 1
   right: 0
  ```

- Cause: `may_start_on_request` (`config.rs:1275`) admits the start because the model's own
  provider has a daily cap. The pool's ledger comes from `spend_policy` (`relaying.rs:211-218`),
  which takes the first billed provider in name order that declares any ceiling. That is
  `other`'s total cap, so `runpod`'s daily cap is never checked.
- Sentences held:
  - the `may_start_on_request` doc comment: "a call starts a billed pod only under a daily cap of
    the account that pays for it";
  - the story's Outcome: "never starts a billed pod unless … names a daily spend cap";
  - correction decision 8: the rule holds where a start is decided.
- What reaches it: nothing in the binary. The loader allows a ceiling on one provider only, and
  `start_connected`/`start_rest` narrow to one provider. Only a library caller of
  `pod_targets`/`start_relaying` reaches it, the same class as pass-1 finding 8. INFEASIBLE.
- Origin: introduced.

**Green.** `crates/llm-gateway-cli/tests/relaying.rs:1999`
`adv2_w02_a_built_deployment_with_the_daily_cap_on_another_provider_lets_no_call_start_a_pod`. It
is green now.

- The case keeps the key, moves the daily cap from `runpod` to another provider, and asks for
  `small`. It asserts `Stopped`, 0 creates and 0 spend reads.
- It is the only test that kills mutant M6 below.

## 3. Suite runs, after the cases existed (worktree, build node)

- Before, with my cases deselected: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked
  -p b10x-llm-gateway-cli --no-fail-fast -- --skip adv2_w02` gave 550 passed, 0 failed, 3
  filtered (my three), exit 0.
- After: the same command without `--skip` gave 551 passed, 2 failed (the two reds above), exit
  101.
- The slowest binary took 12.01 s. Each of my cases takes under 1 s.

## 4. Mutants

Each mutant ran in a scratch clone of 28a81acb with my two test files copied in. The whole crate
suite ran with `--no-fail-fast`. My two reds fail under every mutant, so they are left out of the
"killed by" lists.

| Mutant | Line changed | Result | Killed by |
|---|---|---|---|
| M1 (pass-1 `mutant1.diff`, adapted to the head) | `relaying.rs:390`: `may_start_on_request(..) \|\| provider has daily cap` | killed | `adv_w02_a_daily_cap_without_the_key_lets_no_call_start_a_pod_and_the_wake_still_does`, `adv_w02_a_second_model_that_sets_the_key_starts_only_its_own_pod` |
| M2 (pass-1 `mutant2.diff`, applied unchanged) | `spend_policy` billed filter; `start_connected` provider retain | killed | `adversary_w12p2_another_providers_ceiling_does_not_bind_the_connected_account` (the rewritten test) |
| M3 (`may_start_on_request` ignores the cap) | `config.rs:1276-1279` | killed | `start_on_request_true_without_a_daily_cap_on_the_models_provider_is_a_value_refusal`, `start_on_request_true_with_a_daily_cap_only_on_another_provider_is_a_value_refusal`, `start_on_request_without_a_daily_cap_in_a_built_deployment_lets_no_call_start_a_pod`, my green case |
| M4 (`may_start_on_request` takes any provider's cap) | `config.rs:1277-1279` | killed | `start_on_request_true_with_a_daily_cap_only_on_another_provider_is_a_value_refusal` (loader), my green case |
| M5 (`relayed_models` reads the key directly) | `relaying.rs:390` | killed | `start_on_request_without_a_daily_cap_in_a_built_deployment_lets_no_call_start_a_pod`, my green case |
| M6 (`relayed_models` takes any provider's cap; relay only) | `relaying.rs:390` | survives the unit's 550 tests | only my green case. The loader's tests cannot see a mutant confined to the relay |

Nothing the dropped fifth pass-1 case asserted is now unheld: the rewritten w12p2 test asserts
202, no spend read, not `spend-ceiling` and one create, and it kills M2.

## 5. Judgement findings (28a81acb)

1. **The `contracts/schema/` files still carry the old `upstream-failed` message.**
   `contracts/schema/schema/types/llm-gateway.gateway.Refusal.schema.json:160` and
   `…/entities/llm-gateway.telemetry.UsageRecord.schema.json:233`. They were not regenerated, by
   rule; the coordinator regenerates them with the ESS pin. CONFIRMED, introduced, note.
2. **The restore-paragraph test cannot fail on a wrong sentence.**
   `crates/llm-gateway-cli/tests/config.rs:2000` checks only that the three key strings appear in
   the first paragraph anywhere in the file that contains `start_on_request = true`. By reading:
   - it passes on a paragraph that names the keys while saying the opposite;
   - after a release it still finds the paragraph under the version heading;
   - a later entry that names `start_on_request = true` above it would be read instead.

   Its document half proves the instruction only for a one-provider document (see red 1).
   CONFIRMED, introduced, note.

## 6. Attacked and could not break

- No create site other than `Inner::start` (`pool.rs:1145`), reached only with an authorization.
  `Retry` is never issued by the pool. Profile fallback runs inside a `Provision` only.
- `start_connected`, `start_rest` and the conformance runner (`parse` then `pod_targets`) all go
  through `relayed_models` and so through `may_start_on_request`. No source line other than
  `may_start_on_request` reads `model.start_on_request`; `relaying_stop_tests.rs:46` only sets
  the field in a test.
- `daily_cap_usd = 0`: the loader refuses it (`micro_usd` needs a value above 0). A built
  deployment with `Some(0)` refuses every start by `admit`. A model without a price is `Unknown`,
  so the start is refused `spend-ceiling`.
- The rewritten w12p2 fixture is the same as the dropped fifth case's.
- Docs: `docs/gateway.md` step 7 (the stop window sentence matches the code: `ensure_running`
  then `Unavailable`), `docs/hosting.md:604-626` and `:741-746`,
  `website/docs/concepts/relay.md`, `website/docs/concepts/hosting.md:47-53`,
  `website/docs/reference/deployment-document.md:94`, `docs/model-profiles.md:21-25`, the refusal
  table and `refusals.md`. Outside `contracts/schema/` and `.engineering/`, no file still quotes
  "asks for a replacement".
- The dashboard has no copy of the refusal message.

## 7. Paths written outside the worktree

All under `<scratch>/`:

- Scripts: `run-cases.sh`, `mutants.sh`, `run-suite.sh`
- Mutant diffs: `m1-cap-counts-as-key.diff`, `m2-foreign-ceiling.diff` (a copy of pass-1
  `mutant2.diff`), `m3-ignore-cap.diff`, `m4-any-providers-cap.diff`, `m5-relay-reads-key.diff`,
  `m6-relay-any-providers-cap.diff`
- Logs: `run1-relaying.log`, `run1-config.log`, `m1-…-suite.log` through `m6-…-suite.log` (6
  files), `suite-before.log`, `suite-after.log`, `clippy.log`
- The clone `mut/` and its build output were deleted.

```findings
- file: CHANGELOG.md
  line: 12
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the restore instruction (also README.md:213) asks a price only of the models of the capped provider, but the loader asks one of every model once any provider declares a ceiling, so a pre-change document with a second provider is refused config:value after following it (red in tests/config.rs:2084)
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 211
  category: acceptance
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: may_start_on_request admits a call start on the model's own provider's daily cap while spend_policy binds the first billed provider in name order with any ceiling, so a built deployment with two metered providers starts a pod past that daily cap (red in tests/relaying.rs:2050); the loader admits one metered provider, so only a library caller reaches it
- file: crates/llm-gateway-cli/src/relaying.rs
  line: 390
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: a mutant of relayed_models that reads any provider's daily cap survives the unit's 550 tests and is killed only by the adversary's green case with the cap on another provider in a built deployment
- file: contracts/schema/schema/types/llm-gateway.gateway.Refusal.schema.json
  line: 160
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the generated schema (and UsageRecord.schema.json:233) still carries the old upstream-failed message, awaiting the coordinator's regeneration
- file: crates/llm-gateway-cli/tests/config.rs
  line: 2000
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the restore-paragraph test checks only that three key strings occur in the first paragraph naming start_on_request = true, so it passes on a sentence saying the opposite and reads a later entry that names the key instead
```
