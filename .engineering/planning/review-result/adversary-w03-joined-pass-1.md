---
format: aep.planning-md/3
id: review-result:adversary-w03-joined-pass-1
kind: review-result
status: active
title: Adversary on the joined wave 2026-11-25-w03, pass 1
relations:
- reviews: story:ollama-route
- reviews: story:call-id-and-usage-record
- reviews: story:cost-per-call
revision: 1
---
unit: wave 2026-11-25-w03 (story:ollama-route, story:call-id-and-usage-record, story:cost-per-call)
verdict: red
cases: executed 983→990, red 5
origin: introduced 6, pre-existing 1, undecided 0
wrote-outside-worktree: <scratch>/calls.rs.orig, <scratch>/cli-observability.rs.orig, <scratch>/calls-append.rs, <scratch>/calls-append-2.rs, <scratch>/cli-append.rs, <scratch>/run-1-gateway-cases-alone.log, <scratch>/run-2-cli-case-alone.log, <scratch>/run-3-gateway-suite.log, <scratch>/run-4-cli-suite.log, <scratch>/run-5-gateway-deselected.log, <scratch>/run-6-cli-deselected.log, <scratch>/run-7-clippy-gateway.log, <scratch>/run-8-clippy-cli.log, <scratch>/run-9-mutant-killers-alone.log, <scratch>/run-10-gateway-suite-final.log, <scratch>/run-11-clippy-gateway-final.log, <scratch>/run-m1.log, <scratch>/run-m2.log, <scratch>/run-m1-killers.log, <scratch>/run-m2-killers.log, <scratch>/run-m3-refused-cost.log, <scratch>/run-m4-1023.log, <scratch>/run-m5-zero-count.log, <scratch>/run-m6-swap-classes.log, <scratch>/review.md
needs-coordinator: yes (F3: whether the byte relay must treat a relayed wire `error` event as the end of a call for its cost; F4: route the tool-argument misread, whose counter half reproduces on b6bb227d)

The findings cover branch adv/2026-11-25-w03 at 7d386d81 plus my uncommitted cases. `<scratch>` is `.cache/scratch/6143ef5f-…/gw/w03/adv2` under the operator's home.

## 1. Diff stat

```
 crates/llm-gateway-cli/tests/observability.rs   |  62 +++++
 crates/llm-gateway/tests/observability/calls.rs | 354 ++++++++++++++++++++++++
 2 files changed, 416 insertions(+)
```

Both paths are test files. The changes are additions only: two modules at the end of `calls.rs` and one test at the end of the CLI `observability.rs`. I added no test binary.

The brief allowed mutants in the worktree, while the procedure says to mutate a copy. I followed the brief. I applied six mutants one at a time, to `crates/llm-gateway/src/server.rs:671`, `crates/llm-gateway/src/calls.rs:36` and `crates/llm-gateway-cli/src/pricing.rs:129-130,209`. I reverted each one right after its run. For M1 the revert was `git checkout` of `server.rs`, which had no other uncommitted change; the later reverts were `sed`. `git status` now shows only the two test files.

`cargo fmt --all --check` is clean. Clippy `-D warnings` is clean on both changed test targets.

## 2. Cases added

All run against the fake upstream on loopback, port 0, and send nothing to 11434 or 8091. Each case finished in 0.01 to 0.02 s; none is over 5 s.

### Red now, in `crates/llm-gateway/tests/observability/calls.rs`, module `calls::adversary_w03`

Run alone: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked -p b10x-llm-gateway --test observability adversary_w03` (`<scratch>/run-1-gateway-cases-alone.log`, exit 101, run on build-1d).

**a. `adv_w03_a_stream_the_upstream_cuts_short_has_no_cost`**
- Setup: a model on the messages wire, with a pricing that answers `0.5` whenever input and output were reported. The upstream sends `message_start` (input 12, output 1) and one delta, then closes without the last chunk.
- Asserts: the client's answer lacks the last chunk, and the record has no `cost_usd`.
```
thread 'calls::adversary_w03::adv_w03_a_stream_the_upstream_cuts_short_has_no_cost' (583064) panicked at crates/llm-gateway/tests/observability/calls.rs:1143:9:
assertion `left == right` failed: a stream the upstream cut short after message_start was priced: UsageRecord { call: "5c863c8066a284f6f03e57ce2f941d85-3", model: Some("priced"), wire: Messages, disposition: Relayed, outcome: Ok, refusal: None, status: 200, target_status: Some(200), reported_model: None, input_tokens: Some(12), output_tokens: Some(1), cached_input_tokens: None, cache_creation_input_tokens: None, reasoning_output_tokens: None, cost_usd: Some("0.5"), response_bytes: 287, duration_ms: 0, profile: None, principal: None }; the pricing was asked [Usage { input_tokens: Some(12), output_tokens: Some(1), cache_read_tokens: None, cache_creation_tokens: None }]
  left: Some("0.5")
 right: None
```

**b. `adv_w03_a_passthrough_stream_the_upstream_cuts_short_has_no_cost`**
- Setup: the same cut, on a client-credential model behind `IngressAuth::None`.
```
thread 'calls::adversary_w03::adv_w03_a_passthrough_stream_the_upstream_cuts_short_has_no_cost' (583060) panicked at crates/llm-gateway/tests/observability/calls.rs:1178:9:
assertion `left == right` failed: a passthrough stream the upstream cut short was priced: UsageRecord { call: "5c863c8066a284f6f03e57ce2f941d85-2", ... input_tokens: Some(12), output_tokens: Some(1), ... cost_usd: Some("0.5"), response_bytes: 287, ... }; the pricing was asked [Usage { input_tokens: Some(12), output_tokens: Some(1), cache_read_tokens: None, cache_creation_tokens: None }]
  left: Some("0.5")
 right: None
```

**c. `adv_w03_a_relayed_stream_ended_by_the_wires_error_event_has_no_cost`**
- Setup: a byte-relayed messages stream: `message_start`, one delta, then `event: error` (`overloaded_error`) and a well-framed last chunk.
- Asserts: no `cost_usd`.
```
thread 'calls::adversary_w03::adv_w03_a_relayed_stream_ended_by_the_wires_error_event_has_no_cost' (583061) panicked at crates/llm-gateway/tests/observability/calls.rs:1212:9:
assertion `left == right` failed: a stream ended by the wire's error event was priced: UsageRecord { call: "5c863c8066a284f6f03e57ce2f941d85-1", model: Some("priced"), wire: Messages, disposition: Relayed, outcome: Ok, ... input_tokens: Some(12), output_tokens: Some(1), ... cost_usd: Some("0.5"), response_bytes: 383, ... }
  left: Some("0.5")
 right: None
```

**d. `adv_w03_a_tool_calls_arguments_are_not_the_answers_usage_and_are_not_priced`**
- Setup: a non-streamed messages answer with a `tool_use` whose `input` is `{"input_tokens":900000,"output_tokens":800000}`. Its `usage` is `{"input_tokens":20,"output_tokens":5}`. The pricing writes the counts it is given.
- Asserts: the record reads 20 and 5, and the pricing was given 20 and 5.
```
thread 'calls::adversary_w03::adv_w03_a_tool_calls_arguments_are_not_the_answers_usage_and_are_not_priced' (583065) panicked at crates/llm-gateway/tests/observability/calls.rs:1257:9:
assertion `left == right` failed: the usage the answer reported is 20 input and 5 output tokens: UsageRecord { ... input_tokens: Some(900000), output_tokens: Some(800000), ... cost_usd: Some("0.900000800000"), response_bytes: 256, ... }
  left: (Some(900000), Some(800000), Some("0.900000800000"))
 right: (Some(20), Some(5), Some("0.205"))
```

(The line numbers above are from the run before `rustfmt` and before the second module was added. The tests now start at `calls.rs:1122`, `:1157`, `:1193` and `:1237`.)

### Red now, in `crates/llm-gateway-cli/tests/observability.rs`

**e. `adv_w03_the_usage_event_the_site_pages_show_is_one_the_binary_writes`** (line 464)
- Setup: renders the record the getting-started run makes through `TracingRecords`.
- Asserts: the event equals the line in both site pages, field for field after the timestamp, and the counter part of the id is `1`. The page says it shows the run's only model call.
- Run alone: `cargo test --locked -p b10x-llm-gateway-cli --test observability adv_w03` (`<scratch>/run-2-cli-case-alone.log`, exit 101).
```
thread 'adv_w03_the_usage_event_the_site_pages_show_is_one_the_binary_writes' (1307291) panicked at crates/llm-gateway-cli/tests/observability.rs:512:9:
assertion `left == right` failed: getting-started.md: the page's event against the binary's for the same record (left: the page)
  left: ("usage: usage record call=3f9c0e5a7d1b42c8a6e4f0b9d2c71a85-a model=\"small\" wire=chat disposition=Refused refusal=target-unavailable status=503 response_bytes=0 duration_ms=0", Some("a"))
 right: ("usage: usage record call=3f9c0e5a7d1b42c8a6e4f0b9d2c71a85-a model=\"small\" wire=chat disposition=Refused outcome=refused refusal=target-unavailable status=503 response_bytes=0 duration_ms=0", Some("1"))
```

### Green now, in `calls.rs`, module `calls::adversary_w03_cost_conditions`

Each of these kills one surviving mutant (section 4, M1 and M2). Run alone: `<scratch>/run-9-mutant-killers-alone.log`, 2 passed, exit 0.

**f. `adv_w03_a_translated_answer_ended_in_its_wires_terminal_error_has_no_cost`**
- Setup: a translator writes the head and `message_start`, then calls `TranslatedAnswer::failed(UpstreamFailed)`.
- Asserts: no cost.

**g. `adv_w03_a_call_whose_client_left_mid_answer_has_no_cost`**
- Setup: the client reads `message_start` and closes. The translator waits on `gone()`.
- Asserts: no cost.

## 3. Suite runs (after the cases existed)

**`b10x-llm-gateway`**
- Final command: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked --no-fail-fast -p b10x-llm-gateway` (`<scratch>/run-10-gateway-suite-final.log`, exit 101, build-1d).
  ```
  test result: FAILED. 28 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.54s
  error: test failed, to rerun pass `-p b10x-llm-gateway --test observability`
  error: 1 target failed:
  ```
  Over all targets: 378 passed, 4 failed (a to d), so 382 executed.
- Before: `… -p b10x-llm-gateway -- --skip adversary_w03` gave 376 passed, exit 0 (`run-5`). It ran with cases a to d present and deselected; f and g did not exist yet. 376 + 6 = 382.

**`b10x-llm-gateway-cli`**
- Command: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked --no-fail-fast -p b10x-llm-gateway-cli` (`run-4`, exit 101).
  ```
  test result: FAILED. 7 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
  error: test failed, to rerun pass `-p b10x-llm-gateway-cli --test observability`
  ```
  Over all targets: 607 passed, 1 failed (e), 1 ignored, so 608 executed.
- Before: `-- --skip adv_w03` gave 604 passed, 1 ignored (`run-6`). The filter also deselected three tests of an earlier wave that share the prefix (`adv_w03_a_bedrock_routes_refusals_…`, `adv_w03_credential_set_…` ×2). So before is 604 + 3 = 607.

**Totals and timing**
- Before 376 + 607 = 983, after 382 + 608 = 990, red 5.
- No test of mine is over 5 s. Binaries over 5 s, none mine: `client_credentials` 10.21 s, `gateway` 5.15 s, `wire_relay` 7.58 s, two CLI binaries 12.01 s and 10.17 s.

## 4. Findings

**F1. An answer its upstream cut short is priced** (`story:cost-per-call`). NEEDS-CHANGE, introduced, blocker.
- What was measured: cases a (byte relay) and b (passthrough). Both are red with `cost_usd: Some("0.5")` for a stream cut after `message_start`.
- Cause:
  - `relay_pieces` (`crates/llm-gateway/src/relay.rs:2993`, `Err(_) => return written`) and `pass_answer` (`crates/llm-gateway/src/relay/passthrough.rs:527`, `Err(_) => break`) end on a read error without marking the call.
  - `priced` (`crates/llm-gateway/src/server.rs:671`) checks only the refusal, `failed_after_head` and `client_disconnected`.
- Sentences it holds:
  - docs/gateway.md "The cost of a call": "the call was relayed and its answer ended as written".
  - The deployment reference, `#token-prices`: "A call has a cost only when its answer ended as written".
  - `UsageRecord::cost_usd`: "`None` for a call that was refused, cut short or …".
  - docs/gateway.md relay step 9 names this case "cut short".
  - Story Acceptance 2: never an estimate. The cost here prices 1 output token of an answer that never ended.
- What reaches it: any upstream that drops mid-stream (a pod gone, a reset, a read timeout mid-body takes the same `Err` path) on a priced model. The `message_start` usage of the messages wire is enough.
- Base: `Err(_) => return written` is the same at b6bb227d (`relay.rs:2925`), but the base had no cost. Introduced.
- Fix, named only: mark the call cut short on that `Err` in both writers and leave it out of `priced`.

**F2. The same, on the passthrough route.** Covered by case b at `passthrough.rs:527`. Its row is separate because the line is separate.

**F3. A byte-relayed stream that ends in its wire's `error` event is priced** (`story:cost-per-call`). CONFIRMED, introduced, warning.
- What was measured: case c, `cost_usd: Some("0.5")`, outcome `Ok`.
- Sentences it holds:
  - The `UsageRecord::cost_usd` rustdoc, published API: "`None` for a call that … ended in its wire's terminal error".
  - docs/gateway.md: "no terminal error after the head".
- Contestable: docs/gateway.md "Call outcomes" defines a terminal error after the head for translated answers only (`TranslatedAnswer::failed`). The byte relay does not read the body for it. Hence needs-coordinator.
- What reaches it: Anthropic's mid-stream `overloaded_error` on any messages model relayed byte for byte.

**F4. The scanner reads usage keys inside a tool call's arguments, and the cost prices them** (`story:cost-per-call` with `story:call-id-and-usage-record`). NEEDS-CHANGE, introduced, warning.
- What was measured: case d. The record reads 900000/800000 instead of 20/5, and the pricing was given those counts.
- Cause: `scan` (`crates/llm-gateway/src/usage.rs:99`) takes the maximum of every unescaped `"input_tokens":` and `"output_tokens":` member anywhere in the body. A non-streamed messages `tool_use.input` is a JSON object, not a string.
- Sentences it holds:
  - docs/gateway.md "Counters and usage records": the counters "come from the usage the answer carried".
  - Story Acceptance 2 of cost-per-call, and Acceptance 2 of call-id ("never an estimate").
- What reaches it: a model calling a tool whose arguments carry those key names, on the messages wire, non-streamed. This includes the translated messages answers llm writes, and Ollama's messages answers. Streamed `partial_json` and chat/responses `arguments` are strings, so they are not read.
- Origin: the counter misread reproduces at b6bb227d (`usage.rs` unchanged; base `server.rs:656-659` fills the counters). Pricing those counts, and showing them in the new call record, is this wave's, so I count it introduced by exposure. The coordinator may split it.

**F5. The site pages' usage event omits `outcome=refused`** (contract drift). CONFIRMED, pre-existing, note.
- What was measured: case e.
- Where: `website/docs/getting-started.md:143` and `website/docs/guides/scrape-the-counters.md:169`. Each page says its output was "pasted from a run of the binary". The binary writes `outcome=` between `disposition=` and `refusal=` (`crates/llm-gateway-cli/src/logging.rs:63`).
- Base: the page and the binary differ the same way at b6bb227d.

**F6. The hand-added `call=` on those pages shows a counter of `a`** (`story:call-id-and-usage-record`). CONFIRMED, introduced, note.
- What was measured: case e.
- The getting-started run makes one model call, and the page says "here the refused one". A process's first id ends `-1` (docs/gateway.md: "a counter in lowercase hexadecimal that starts at 1").
- `-a` matches `accepted=10`, which counts connections, not model calls. The binary cannot write this line for that run.

**F7. Two conditions of `priced` were held by no test** (mutant). CONFIRMED, introduced, note.
- Where: `crates/llm-gateway/src/server.rs:671`, mutants M1 and M2 below, which survived the whole gateway suite.
- The tests that should have failed: none existed. My green cases f and g now kill them.

**Judgement, no case**
- `server.rs` ends the ticket after the last chunk is written (`record` runs after `relay::serve` returns). A client that parses the chunked end and reads `GET /v1/calls/{id}` at once may still see `in-flight`.
- docs/gateway.md promises only "before the connection is closed". Story Acceptance 3 says "once the last byte is written it is ended".
- I could not make this deterministic without a fixture of my own that delays the target's drop, so it is INFEASIBLE as a case. Not in the block.

## 5. Mutants

| Mutant | Line changed | Run | Result |
| --- | --- | --- | --- |
| M1 | `server.rs:671`, drop `call.failed_after_head.is_none() &&` | gateway suite minus my cases (`run-m1`) | 376 passed, survived; killed by case f (`run-m1-killers`) |
| M2 | `server.rs:671`, drop `&& !call.client_disconnected` | `run-m2` | 376 passed, survived; killed by case g (`run-m2-killers`) |
| M3 | a refused call carries a cost: `server.rs:671`, drop `refusal.is_none() &&` | `run-m3-refused-cost` | killed by `cost_a_refused_call_and_an_upstream_failure_of_a_priced_model_carry_none_and_ask_nothing` |
| M4 | keep 1023: `calls.rs:36`, `MAX_ENDED_CALLS` 1024→1023 | `run-m4-1023` | killed by `a4_after_1025_ended_calls_…`, `every_numeric_bound_in_the_source_is_published`, `every_published_bound_flips_at_exactly_the_published_number` |
| M5 | drop the zero-count rule: `pricing.rs:209`, `cached_input_tokens: usage.cache_read_tokens` | `run-m5-zero-count` | killed by `cost_a_count_of_zero_in_a_class_without_a_rate_keeps_the_cost` and 4 others |
| M6 | swap two token classes: `pricing.rs:129-130`, `cached_input` ↔ `cache_creation_input` | `run-m6-swap-classes` | killed by `cost_cache_read_…`, `cost_cache_creation_…`, `cost_every_class_of_one_answer_is_summed` |

Not run, killed by reading:
- **A passthrough answer given the upstream's id:** `a1_a_passthrough_answer_carries_the_gateways_id_and_never_the_upstreams`, through `Answer::call`'s at-most-one assertion and `assert_ne!`.
- **Skip `/api/ps`:** pass 1's reading holds: `ollama_status_is_read_from_tags_then_ps_and_sends_nothing_else`.

## 6. Attacked and could not break

- **Call store:** keyed by unique id, its lock held only to copy or change an entry, eviction FIFO over ended ids, and the 1025th call held correctly (M4).
- **Id form and length:** at most 49 characters. Distinct per process (OS-seeded `RandomState`). Never taken from the upstream: the byte relay passes only allowlisted lines, and passthrough drops a lowercased `x-llm-gateway-call`.
- **Translated wire tokens:** the scanner reads the client-wire bytes and prices them with the client wire. llm's chat and responses encoders write prompt tokens inclusive and `cache_creation_input_tokens` as a named extension, which the scanner reads. llm's messages encoder writes `input_tokens` only when both cache counters are known. So no double count and no wrong-side count.
- **Route price mapping and USD refusal:** an llm-refused amount names `price.<field>` without the value.
- **Ollama `model-not-pulled`:** comes from a 2xx list only; a 5xx is `status-<code>` and is relayed.
- **Streamed chat without `include_usage`:** has no tokens and no cost.

## 7. Paths written outside the worktree

All are under `<scratch>`:
- Copies of the test files before my edits: `calls.rs.orig`, `cli-observability.rs.orig`.
- The appended case text: `calls-append.rs`, `calls-append-2.rs`, `cli-append.rs`.
- Run logs: `run-1` to `run-11` and `run-m1` to `run-m6` (with the two `-killers` logs).
- This report: `review.md`.

```findings
- file: crates/llm-gateway/src/relay.rs
  line: 2993
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a byte-relayed answer the upstream cut short is not marked, so its partial message_start counts are priced and the record carries cost_usd (case adv_w03_a_stream_the_upstream_cuts_short_has_no_cost)
- file: crates/llm-gateway/src/relay/passthrough.rs
  line: 527
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a passthrough answer the upstream cut short is priced the same way (case adv_w03_a_passthrough_stream_the_upstream_cuts_short_has_no_cost)
- file: crates/llm-gateway/src/server.rs
  line: 671
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a byte-relayed stream ended by the wire's error event is priced, against the UsageRecord::cost_usd rustdoc's "ended in its wire's terminal error" (case adv_w03_a_relayed_stream_ended_by_the_wires_error_event_has_no_cost)
- file: crates/llm-gateway/src/usage.rs
  line: 99
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the usage scanner reads input_tokens and output_tokens members inside a non-streamed messages tool_use input, and the new cost prices those counts; the counter misread itself reproduces at b6bb227d (case adv_w03_a_tool_calls_arguments_are_not_the_answers_usage_and_are_not_priced)
- file: website/docs/getting-started.md
  line: 143
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the usage event shown on getting-started and scrape-the-counters as pasted from a run lacks the outcome=refused field the binary writes (case adv_w03_the_usage_event_the_site_pages_show_is_one_the_binary_writes)
- file: website/docs/getting-started.md
  line: 143
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the hand-added call id on both pages ends in counter a, but the run's only model call would carry counter 1
- file: crates/llm-gateway/src/server.rs
  line: 671
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: dropping failed_after_head or client_disconnected from priced survived the whole gateway suite; cases adv_w03_a_translated_answer_ended_in_its_wires_terminal_error_has_no_cost and adv_w03_a_call_whose_client_left_mid_answer_has_no_cost now kill them
```
