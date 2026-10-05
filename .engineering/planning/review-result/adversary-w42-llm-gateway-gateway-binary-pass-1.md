---
format: aep.planning-md/3
id: review-result:adversary-w42-llm-gateway-gateway-binary-pass-1
kind: review-result
status: active
title: Wave 2026-10-05-w42 adversary, llm-gateway story:gateway-binary, pass 1
relations:
- reviews: story:gateway-binary
revision: 1
---
```
unit: beyond10x/llm-gateway story:gateway-binary, worktree llm-gateway-w42-gateway-binary at ff4ec33 (base ccf3501) plus untracked tests/adversary.rs
verdict: NEEDS-CHANGE
cases: executed 35→44, red 4
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 (build dir, deleted)
needs-coordinator: none
```

Cases added: `crates/llm-gateway-cli/tests/adversary.rs` (9 cases; 4 red: a 4096-byte owner secret with
its newline, a refused start with a broken stderr pipe, a signalled stop after stderr breaks, SIGTERM
against a trickling client).

Not broken: final-component symlink refused; checks on the opened handle; directory, FIFO and
device refused without blocking; size bound on bytes read; hard links not exploitable under
protected hard links; unknown and duplicate keys, wrong types and NaN refused; empty models
refused; owner secret compared in constant time over SHA-256 digests; refusals never quote the
secret; clap errors exit 2.

```findings
[
{"file":"crates/llm-gateway/src/server.rs","line":240,"category":"concurrency","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a client that sends its request head one byte a second keeps a SIGTERM-triggered shutdown waiting on that connection for up to 8192 x 10 s, and a second signal is swallowed, so D2's graceful stop has no bound"},
{"file":"crates/llm-gateway-cli/src/main.rs","line":26,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"eprintln! panics on a broken stderr pipe, so a refused start exits 101 instead of 1 and a signalled stop exits 101 instead of 0"},
{"file":"crates/llm-gateway-cli/src/trusted.rs","line":76,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the 4096-byte limit applies before trimming, so a 4096-byte owner secret with a trailing newline is refused as too-large although the README admits tokens up to 4096 bytes"},
{"file":"docs/llmgw-capability-matrix.md","line":115,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"K30 says the owner rule cannot be tested without another user, but a user namespace mapping to another uid tests both the untrusted-owner refusal and the root trust"},
{"file":"crates/llm-gateway-cli/src/config.rs","line":328,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"idle_timeout_minutes admits 0 while RunpodModel refuses a zero idle_timeout_ms, a conflict that surfaces only when a later story maps one onto the other"}
]
```
