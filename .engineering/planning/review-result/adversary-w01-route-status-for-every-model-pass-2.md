---
format: aep.planning-md/3
id: review-result:adversary-w01-route-status-for-every-model-pass-2
kind: review-result
status: active
title: 'Adversary, pass 2 (story:route-status-for-every-model, wave 2026-11-25-w01): red, 1 introduced red case of note severity, 2 green cases added, 1 mutant the suite missed'
relations:
- reviews: story:route-status-for-every-model
revision: 1
---
The adversary's report of pass 2 on `impl/route-status-for-every-model` at a3c563e3, as it
returned. Paths of the coordinator's scratch directory are written `<scratch>/`.

```
unit: story:route-status-for-every-model (head a3c563e3 plus one untracked test file in the worktree)
verdict: red
cases: executed 550→553, red 1
origin: introduced 2, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/{case-alone.log, suite.log, run-mutant.sh, count.sh, m1-guide-kind.diff, m1-guide-kind.log, m2-late-opening-used.diff, m2-late-opening-used.log, m3-second-opening.diff, m3-second-opening.log}; the mutant clone adv2/mut was deleted
needs-coordinator: no
```

The correction mostly holds. I wrote one red case: a connection that an `https` opening makes
after the deadline stays open until the next status read of that model, which may never come.
With the shipped TLS connector this only happens in a window of a few milliseconds before the
deadline, so it is a note, not a blocker. One mutant (a late opening's connection used by the
next probe) passes every test the unit wrote; a green case of mine catches it. Nothing else
broke.

## 1. What I touched

`git --no-pager diff --stat` prints nothing. `git status --short`:

```
?? crates/llm-gateway-cli/tests/adversary_w01_route_status_pass2.rs
```

It is a test file and is not committed. I changed no implementation file in the worktree. Mutants
were applied only in a scratch clone, which is now deleted.

## 2. Cases added

All three are in `crates/llm-gateway-cli/tests/adversary_w01_route_status_pass2.rs`. They use
`HostedTargets` with an `https` endpoint and a `PodConnector` written in the test, which stands in
for a TLS handshake that ends after the probe deadline. Every endpoint is a loopback listener on
port 0. The deadline is 300 ms and the reuse time 0. The whole file runs in 2.30 s and no case
takes over 5 s.

**a) `route_status_adv2_a_connection_an_opening_made_after_the_deadline_is_not_held_open`: red now.**

- It reads the status once (`no-answer` at the deadline), then lets the opening finish: it
  connects to the endpoint. The case asserts that the endpoint sees the connection closed within
  2 s.
- The sentence it holds, from `docs/gateway.md`, "Bounds of the status probe": "once it has
  ended, what it found is dropped".
- Red output from the run of this file alone, before any suite run (`case-alone.log`):

```
test route_status_adv2_a_connection_an_opening_made_after_the_deadline_is_not_held_open ... FAILED
thread 'route_status_adv2_a_connection_an_opening_made_after_the_deadline_is_not_held_open' (2429471) panicked at crates/llm-gateway-cli/tests/adversary_w01_route_status_pass2.rs:175:5:
assertion `left == right` failed: the connection opened after the deadline is still held open 2 s after its opening ended
  left: Err(Timeout)
 right: Ok(ClosedIdle)
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s
```

- **Cause:** `hosted.rs:728-732`. At the deadline the receiver is kept in `left_running`. The
  opening thread then sends its `Ok(stream)` into that channel, where it stays until the next
  `opened()` call drops it.
- **What reaches it with the shipped connector (`tls.rs:145-172`):**
  - The connect and the handshake are bounded by a deadline equal to the probe's.
  - After the last handshake read, the certificate check and the socket timeouts are not checked
    against that deadline.
  - On `http`, a connect that ends in the same instant the probe stops waiting hands over its
    socket the same way.
  - So it happens only when the opening ends in the last milliseconds before the deadline. A
    `PodConnector` that bounds each step but not the total widens that window; the trait is
    public.
- **Cost:** one idle connection per model (no request sent, no credential sent), held until the
  model's next status read.
- **Origin:** introduced. `left_running` and the opening thread are new in a3c563e3.
- **Fix, named and not applied:** in the opening thread, drop what was opened instead of sending
  it once `Instant::now() >= deadline`. Or the docs say "dropped at the next probe".

**b) `route_status_adv2_the_next_probe_never_uses_a_late_opening`: green now.**

- The late opening connects to an endpoint that answers 200; a fresh opening connects to one that
  answers 503. The next read must say `status-503`, call the connector a second time, and the
  late connection must be closed without a request.
- It holds "what it found is dropped and the next probe looks the name up again".
- It catches mutant M2 below, which every test the unit wrote lets pass.

**c) `route_status_adv2_an_https_opening_that_never_ends_is_no_answer_at_each_deadline`: green now.**

- This covers the gap the implementor reported ("A never-returning https lookup is not tested").
- Two reads are each `no-answer` between 300 ms and 800 ms, and the connector is called once.

## 3. Suite run, after the cases existed

Command, run in the worktree on the build node:

```
RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked --no-fail-fast -p b10x-llm-gateway-cli
```

It exited 101. Log: `<scratch>/suite.log`.

```
     Running tests/adversary_w01_route_status_pass2.rs (target/debug/deps/adversary_w01_route_status_pass2-852452d38ac100f5)
running 3 tests
test route_status_adv2_the_next_probe_never_uses_a_late_opening ... ok
test route_status_adv2_an_https_opening_that_never_ends_is_no_answer_at_each_deadline ... ok
test route_status_adv2_a_connection_an_opening_made_after_the_deadline_is_not_held_open ... FAILED
test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.30s
```

- Totals over all `test result` lines: 551 passed, 1 failed, 1 ignored, so 553 executed.
- "Before" is 550: the same run with my file's 3 cases taken out. I made no separate run for it.
- No other test failed. The slowest binary took 12.01 s for 9 tests.

## 4. Mutants (scratch clone, deleted; diffs and logs kept)

- **M1, `guide.rs:229`:** `(model.kind() == ModelKind::Pod)` → `(… || alias != "")`. **Killed** by
  three tests:
  - `lifecycle_routes.rs::route_status_the_launch_route_asks_the_status_of_a_pod_model_only` (:782)
  - `wake_status.rs::route_status_the_launch_route_asks_no_endpoint` (:788)
  - `wake_status.rs::route_status_the_launcher_starts_claude_without_a_probe_of_its_routes` (:835)
- **M2, `hosted.rs:711-717`:** when the opening left running has ended with `Ok(stream)`, return
  that stream instead of opening afresh. **Survived** the unit's tests (`--lib route_status`: 10
  ok; `--test wake_status route_status`: 5 ok).
  - Killed by my case (b) at :204: `left: Some(ModelStatus { phase: Ready … })`, `right: …
    status-503`.
  - The unit's tests miss it because on `http` a late opening always ends in `Err`: `step()?`
    fails once the deadline has passed. No test of theirs reaches a late `Ok`.
- **M3, `hosted.rs:713`:** do not wait for the opening left running; start a second one (`… &&
  false`). **Killed** by:
  - `hosted::tests::route_status_a_lookup_past_the_deadline_is_no_answer_and_only_one_is_left_running` (:1651)
  - `wake_status.rs::route_status_a_lookup_that_never_returns_is_no_answer_and_the_list_keeps_its_bound`
    (:629, "a second lookup started")
  - my case (c)

Judgement findings: none beyond the two above.

## 5. Attacked and not broken

- **`launch.rs`:** identical to the base (`git diff d71a9cac a3c563e3` is empty for it).
- **Launch route:** it asks only pod models, which is what it effectively asked at the base,
  since hosted models, routes and Bedrock models had no status there.
- **Composition paths:** every one sets its kind: `relaying.rs:1211` Hosted, `:1252` Bedrock,
  `routes.rs:333` Route. No builder resets it, so no hosted model, route or Bedrock model is left
  at the `Pod` default.
- **Deadline of one probe:** the `http` connect to each address is bounded by `connect_by` ≤
  deadline, the exchange by the deadline, and the probe's own wait by `recv_timeout`.
- **Openings and the per-model lock:**
  - At most one opening per model is left running, because the per-model lock keeps
    `left_running` access in one order (`probed`, then `left_running`).
  - The opening thread holds only `Arc`s, never the lock.
  - A failed thread spawn answers `no-answer`.
  - Neither a stale `no-answer` nor a late `ready` survives past the reuse time.
- **`credential set`:** same bound, errors and output, and its tests are unchanged. The only
  difference is that the connect window now starts after the lookup instead of before it. That is
  still inside `PROBE_TIMEOUT` per step and `PROBE_TOTAL` overall.
- **Contract drift:** none found. `docs/gateway.md` (the launch paragraph and "Bounds of the
  status probe"), `CHANGELOG.md`, the spec, `status.rs` and the website reference agree with the
  code, apart from finding (a).

## 6. Paths written outside the worktree

- In `<scratch>/`: `case-alone.log`, `suite.log`, `run-mutant.sh`, `count.sh`,
  `m1-guide-kind.diff`, `m1-guide-kind.log`, `m2-late-opening-used.diff`,
  `m2-late-opening-used.log`, `m3-second-opening.diff`, `m3-second-opening.log`.
- The clone `adv2/mut` (1.1 GB with its target) was deleted. The pool synced it to the build
  node; any copy there is the pool's, and I did not touch it.
- The suite's build output is in the worktree's `target/`.

```findings
- file: crates/llm-gateway-cli/tests/adversary_w01_route_status_pass2.rs
  line: 175
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: an opening that ends after the probe deadline with a connection hands it into the kept channel (hosted.rs:728-732), where it stays open until the model's next status read, against "once it has ended, what it found is dropped"; with the shipped TLS connector only an opening that ends in the last milliseconds before the deadline reaches it.
- file: crates/llm-gateway-cli/src/hosted.rs
  line: 711
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: using a late opening's Ok connection for the next probe survives every route_status test of the unit (an http late opening is always Err), and is caught only by adversary_w01_route_status_pass2.rs::route_status_adv2_the_next_probe_never_uses_a_late_opening.
```
