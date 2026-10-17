---
format: aep.planning-md/3
id: review-result:adversary-w12-relay-read-interrupted-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:relay-read-interrupted, pass 1
relations:
- reviews: story:relay-read-interrupted
revision: 1
---
unit: story:relay-read-interrupted, adversary pass 1, covering adv/relay-read-interrupted at ab33960 (wave head 558fd87 plus one test commit)
verdict: red (CONFIRMED, one finding introduced by the fix)
cases: executed 237→238, red 1
origin: n/a (adversary: introduced 1, pre-existing 0, undecided 0)
wrote-outside-worktree: 7 paths, all in the wave's scratch directory fixadv/: run.sh, msg-red-1.txt, build.log, size.log, case.log, suite.log, clippy.log
needs-coordinator: yes. The fix needs a bound on the target read, and the right bound is a design choice (see the finding).

1. Diff since 558fd87: `crates/llm-gateway/tests/adversary_w03_relay_read_interrupted.rs | 181 +` and nothing else. It is a test file only, and no implementation file was touched.
2. Commit ab33960 `test: a silent target under stops and continues is answered in its read timeout (red)`. The new file holds one case and nothing else, because the stops reach every thread in the test binary.
   - Setup: the target is a loopback listener that never answers. Its connection's read timeout is 400 ms. A `sh` child stops and continues the test process every 25 ms (a deterministic SIGSTOP/SIGCONT).
   - Assertion: the client gets `502 upstream-failed` within 4 s.
   - Red output from running the case alone: `panicked at .../adversary_w03_relay_read_interrupted.rs:170:9: no answer within 4s (4.0000838s waited) from a target whose read timeout is 400ms: each interrupted read restarted it`, then `test result: FAILED. 0 passed; 1 failed`.
3. Suite run after the case existed (`cargo test -p b10x-llm-gateway --locked --no-fail-fast`, exit 101): every other test binary passed, 237 cases in total. The new file gave `test result: FAILED. 0 passed; 1 failed`. Clippy `--all-targets --all-features -D warnings` passed (exit 0). Conformance was not run because no spec changed. The first build grew `target/` to 387 MB.
4. Finding. The target read in `crates/llm-gateway/src/relay.rs:711-726` (`Source::Target`, line 721) is retried with no deadline. The only bound on a target is the connection's own read timeout (`RelayTarget::connect`), and an interrupted read on a socket with a read timeout starts that whole timeout again (`man 7 signal`).
   - So while interruptions keep coming faster than the timeout, a silent target is never timed out. This covers `read_answer_head`, `stream_answer` and the count thread.
   - Before the fix, the first interrupted read ended the request with a 502, which was bounded (`git show cf186de:crates/llm-gateway/src/relay.rs`, `stream.read(&mut chunk)?`). That is why the origin is "introduced".
   - It contradicts the acceptance line "their deadlines still bound them" for `Buffered::fill`.
   - What reaches it: a process that is stopped and continued repeatedly, as in the fix's own reproduction (every 50 ms), or a container that is frozen and thawed. No production caller that does this all the time was found, hence PLAUSIBLE.
   - Fix to name, not applied: give `Source::Target` its own deadline when it is built, or stop retrying an interrupted read past a set time or count.
5. Attacked and not broken:
   - The `Source::Client` retry checks its deadline on every pass, so it is bounded.
   - `read_head` and `linger` check the deadline before each read.
   - An interrupted read is never taken for data or for the end of the stream: it never reaches `Ok(0)` or `Ok(n)`.
   - `gone()` treats an interrupted peek as the client still being there.
   - `write_by` and `write_request` (std `write_all`) retry an interrupted write.
   - Translation reads go through tokio's non-blocking sockets, which handle interruptions themselves. The CLI's TLS seam is outside this gate and was not tested.
6. Paths written outside the worktree are the 7 in the header line. No background process left; lease released.

```findings
[{"file":"crates/llm-gateway/src/relay.rs","line":721,"category":"acceptance","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"The retry of an interrupted target read has no deadline, and each interrupted read restarts the connection's read timeout, so a silent target under repeated stop/continue is never answered (before the fix it was answered 502); plausibility PLAUSIBLE; test crates/llm-gateway/tests/adversary_w03_relay_read_interrupted.rs::a_silent_target_under_stops_and_continues_is_still_answered_within_its_read_timeout"}]
```
