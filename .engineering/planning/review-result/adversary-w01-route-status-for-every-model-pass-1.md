---
format: aep.planning-md/3
id: review-result:adversary-w01-route-status-for-every-model-pass-1
kind: review-result
status: active
title: 'Adversary, pass 1 (story:route-status-for-every-model, wave 2026-11-25-w01): red, 1 introduced red case, 1 warning and 1 note by reading'
relations:
- reviews: story:route-status-for-every-model
revision: 1
---
The adversary's report of pass 1 on `impl/route-status-for-every-model` at f4d64be9, as it
returned. Paths of the coordinator's scratch directory are written `<scratch>/`.

```
unit: story:route-status-for-every-model (head f4d64be9 plus one untracked test file in the worktree)
verdict: red
cases: executed 546→548, red 1
origin: introduced 1, pre-existing 0, undecided 0
wrote-outside-worktree: 11 paths (listed in part 6)
needs-coordinator: yes. Two decisions. First, whether the launcher's own 5 s bound (`src/launch.rs:39`, stated in code, not in `docs/gateway.md`) counts as a contract under the brief's rule. Second, whether to keep my red case, which holds a copy of that 5 s value.
```

## 1. What I touched

`git --no-pager diff --stat` prints nothing, because the only change is a new untracked file.
`git status --short`:

```
?? crates/llm-gateway-cli/tests/adversary_w01_route_status.rs
```

It is a test file. I changed no implementation file and applied no mutant.

## 2. Cases added

File: `crates/llm-gateway-cli/tests/adversary_w01_route_status.rs`. It runs the shipped binary on
loopback, open (`ingress_auth = "none"`), with one hosted model `qwen` (the launcher's default
implementer route). Its endpoint is a loopback listener that accepts and never answers. Each case
runs about 5.0 s.

**a) `route_status_adv_the_launch_route_is_answered_inside_the_launchers_fetch_timeout`: red now.**

- It sends `GET /v1/launch` and asserts that the answer is 200, that it names
  `"phase":"unreachable"`, and that it arrives in under 5 s.
- 5 s is the launcher's `FETCH_TIMEOUT` (`src/launch.rs:39`, documented as "How long the gateway
  may take to answer the launch route").

Red output from the run of this file alone, before any suite run (`<scratch>/case-alone.log`):

```
test route_status_adv_the_launch_route_is_answered_inside_the_launchers_fetch_timeout ... FAILED
thread 'route_status_adv_the_launch_route_is_answered_inside_the_launchers_fetch_timeout' (2397415) panicked at crates/llm-gateway-cli/tests/adversary_w01_route_status.rs:158:5:
GET /v1/launch took 5.002426974s, at least the launcher's whole fetch bound of 5s: `b10x-llm-gateway claude` gives up before the answer
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.01s
```

**b) `route_status_adv_the_launcher_notes_a_silent_implementer_route_and_starts_claude`: green now.**

- It runs `b10x-llm-gateway claude` end to end against that gateway, with
  `tests/fixture/claude.rs` standing in for Claude Code.
- It asserts exit 0 and the note "claude: route qwen is unreachable; its calls fail until its
  endpoint answers".
- It holds the CHANGELOG "Unreleased" sentence "`b10x-llm-gateway claude` notes an `unreachable`
  implementer route…".
- It was green 8 times out of 8: 1 alone, 6 in a loop (`<scratch>/race.log`), 1 in the suite.
- It passes only on a margin of a few milliseconds. The gateway answers at about 5.0024 s. The
  launcher's per-read timeout of 5 s fires slightly later because the kernel rounds socket
  timeouts up. On a loaded host this case can turn red, and when it does, that is this defect.

## 3. Suite run, after the cases existed

Command: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked --no-fail-fast -p
b10x-llm-gateway-cli` in the worktree. It ran on the build node. The log is
`<scratch>/suite2.log`, and the command exited 101.

An earlier run without `--no-fail-fast` (`<scratch>/suite.log`) stopped at my binary, so I do not
use it for the count.

```
     Running tests/adversary_w01_route_status.rs (target/debug/deps/adversary_w01_route_status-9ef43ba5a7aca264)
running 2 tests
test route_status_adv_the_launch_route_is_answered_inside_the_launchers_fetch_timeout ... FAILED
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.02s
error: test failed, to rerun pass `-p b10x-llm-gateway-cli --test adversary_w01_route_status`
```

- Totals over all `test result` lines: 547 passed, 1 failed, 1 ignored, so 548 executed.
- No other test failed.
- "Before" is 546: the same run with my binary's 2 cases taken out of the count. I did not make a
  separate run for it.
- The slowest binaries: 6.72 s, 5.07 s, and mine at 5.02 s.

**The finding: `/v1/launch` now answers after the launcher has stopped waiting.**

- **Measured:** `adversary_w01_route_status.rs:158`. `GET /v1/launch` took 5.0024 s with one
  silent hosted endpoint.
- **What it holds:** the launcher's bound in `src/launch.rs:39`, and through case (b) the
  CHANGELOG sentence above.
- **Origin: introduced.**
  - At base, `guide.rs:227` read `relay.status(alias)`, and `HostedTargets::status` returned
    `None` for a hosted alias (base `hosted.rs:580-585`). So the route was answered with no
    upstream request.
  - Now `guide.rs:200` calls `relay.statuses()`, which probes every hosted model and route with
    the published 5000 ms deadline. That deadline equals the launcher's whole fetch bound.
  - `b10x-llm-gateway codex` uses the same `fetch` and is exposed the same way.
- **What reaches it:** the default launcher run (`claude` or `codex`) against an open gateway with
  any hosted model or route whose endpoint does not answer, or answers `/models` slowly. That
  includes a host that drops connection attempts. After a probe the result is reused for 10 s, so
  a retry right away succeeds; the first launch after an idle period is the exposed one.
- **Fix, named and not applied:** either give `FETCH_TIMEOUT` a value above
  `status-probe-timeout-ms` plus 1 s (derived from it, not a second literal), or let `/v1/launch`
  use a shorter probe deadline or only reused results. If the fix raises `FETCH_TIMEOUT`, change
  the 5 s value in my case (a) with it.

## 4. Judgement findings (cover head f4d64be9)

- **The probe deadline does not cover the DNS lookup.**
  - Where: `crates/llm-gateway-cli/src/hosted.rs:322`, which calls `dial` → `tls.rs:291`
    `(host, port).to_socket_addrs()`. The https path goes through `tls.rs:153` the same way.
  - The lookup runs on the status thread before the deadline applies. A resolver that does not
    answer stretches one probe, and so `/v1/status` and `/v1/launch`, past
    `status-probe-timeout-ms`.
  - That breaks the published `docs/gateway.md` sentence ("Bounds of the status probe"): "One
    probe, connection, handshake, request and first final status, ends within
    `status-probe-timeout-ms`", and the bound of the list at the deadline plus one second.
  - What reaches it: any hosted endpoint or route whose base URL names a host when DNS is slow or
    down.
  - Verdict INFEASIBLE as a case here: it needs a resolver that does not answer, and nothing on
    loopback can stand in for one. Origin: introduced (the `credential set` probe code existed
    before; this change makes a status read reach it). Severity: warning.
- **A panicked status thread is shown as "no status".**
  - Where: `crates/llm-gateway/src/relay.rs:1352`. `thread.join().ok().flatten()` turns a
    panicked thread into `None`.
  - The list then shows that model with neither `phase` nor `callable`. `docs/gateway.md`
    reserves that shape for a model with no status (Bedrock).
  - What reaches it: nothing found. I found no path where a probe panics.
  - Verdict INFEASIBLE. Origin: introduced. Severity: note.

Mutants: none. Rules forbid mutating files under attack, and I did not copy the crate to mutate a
copy. From reading the tests:

- Every arm of `probed_status` is pinned by `hosted.rs`
  `route_status_a_hosted_status_is_ready_on_2xx_and_unreachable_saying_why_otherwise`: 2xx; 401
  and 403 with and without `client`; another status; no answer.
- `callable` per phase is pinned by
  `route_status_callable_is_true_exactly_where_a_call_is_relayed_or_held` and the list test in
  `lifecycle_routes.rs`.

## 5. Attacked and not broken

- **Probe request content:** the exact request head is pinned per `auth_kind`. Reading `probed`
  and `probe_request`, nothing of the caller reaches it.
- **Reuse edges:** at 9,999 and 10,000 ms. Two reads at once send one probe. Both are tested, and
  the code holds the lock for the whole probe.
- **Refusal order:** owner credential, method, body and readiness are all decided before any
  probe (`server.rs:1417-1449`, `status_list`).
- **List bound at the published 5000 ms:** one silent endpoint answered in 5.0024 s, inside 5 s
  plus 1 s.
- **No side effects:** `HostedTargets::status` reaches no `acquire`, `wake` or `invalidate`, and
  records nothing.
- **https:** the TLS handshake is bounded by the deadline (`tls.rs` `open` and `Deadlined`).
- **Wake and Bedrock:** wake stays `path-unknown` for hosted models, and Bedrock's status stays
  `None` (`bedrock.rs:442`).

## 6. Paths written outside the worktree

All in `<scratch>/`: `case-alone.log`, `race.sh`, `race.log`, `race.log.1` through `race.log.6`,
`suite.log`, `suite2.log`.

The tests also created `target/tmp/adversary-w01-status-launch-route` and
`target/tmp/adversary-w01-status-launcher` under the worktree's target directory
(`CARGO_TARGET_TMPDIR`), and their copies on the build node.

```findings
- file: crates/llm-gateway-cli/tests/adversary_w01_route_status.rs
  line: 158
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: GET /v1/launch now probes every hosted endpoint with the 5000 ms deadline and answers a silent endpoint at 5.0024 s, past the launcher's 5 s FETCH_TIMEOUT (src/launch.rs:39), so `claude`/`codex` launch succeeds only by kernel timeout rounding.
- file: crates/llm-gateway-cli/src/hosted.rs
  line: 322
  category: judgement
  severity: warning
  verdict: INFEASIBLE
  origin: introduced
  message: the probe's DNS lookup (dial -> to_socket_addrs) runs outside status-probe-timeout-ms, so a stalled resolver breaks the published probe and list bounds; not testable on loopback.
- file: crates/llm-gateway/src/relay.rs
  line: 1352
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a panicked status thread is rendered as a model with no status (no phase, no callable), the shape docs/gateway.md reserves for Bedrock; no panicking path found.
```
