---
format: aep.planning-md/3
id: review-result:adversary-w10-hosted-base-path-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w01 adversary, story:hosted-base-path, pass 1
relations:
- reviews: story:hosted-base-path
revision: 1
---
```
unit: story:hosted-base-path at 845bc97 (tree lg-w10-adv-base, branch adv/hosted-base-path)
verdict: green
cases: executed 387→392, red 0
origin: introduced 1, pre-existing 0, undecided 0
wrote-outside-worktree: (harness log of one background run, in the session's task directory)
needs-coordinator: no
```

**I couldn't make it fail.** All 5 cases I added pass against 845bc97. The one finding is a mutant the unit's own suite doesn't catch; one of my new tests does catch it.

**1. Diff.** `git --no-pager diff --stat` is empty: both files are new and untracked. Both are test files, and no implementation file was touched.
- `crates/llm-gateway/tests/adversary_w10_base_path.rs`
- `crates/llm-gateway-cli/tests/adversary_w10_base_path.rs`

**2. Cases added (each file run alone first, before the suite)**

| File | Case | What it checks | Result |
|---|---|---|---|
| gateway | `every_byte_outside_the_rule_is_refused_wherever_it_stands` | Each of the 189 disallowed bytes, at 4 positions in a path, is refused `NotAPath` | green 2/2 |
| gateway | `no_accepted_base_path_joins_to_a_path_that_leaves_its_prefix` | Every path up to 6 bytes over `/ . a ~`, joined to all 4 wire paths: the prefix is kept, no `.`/`..`/empty segment, no byte outside the rule | (same run) |
| cli | `an_ipv6_literal_with_a_path_keeps_its_brackets_in_the_authority` | `http://[::1]:8000/api/v1` with a bearer works; `[::1]` stays in the authority | green 3/3 |
| cli | `delimiters_after_the_first_slash_are_refused_as_path_bytes` | `@`, `:`, `;` and `[` after `/`, plus `[::1/v1]`, `[::1]x/v1`, `host:/v1`, `/v1/.` and `/./` are all `Malformed` | (same run) |
| cli | `a_path_neither_opens_nor_closes_the_cleartext_rule` | A bearer or api-key over `http` to a remote host with a path is still `CleartextCredential` | (same run) |

The 3 CLI cases only pin current behaviour. Drop them if you want tests-only-red to hold strictly.

**3. Suite.** I ran `RUSTUP_TOOLCHAIN=1.98.0 RUSTC_WRAPPER=sccache cargo test --locked --no-fail-fast -p b10x-llm-gateway -p b10x-llm-gateway-cli` after the cases existed. It exited 0 with 392 passed and 0 failed. The before count (387) is that run minus my two `adversary_w10_base_path` binaries (2 + 3), not a separate run. Log: `target/lg-w10-adv/suite.log`.

**4. Finding**

| file:line | What was measured | What reaches it | Verdict | Origin |
|---|---|---|---|---|
| `crates/llm-gateway/src/relay.rs:208` | I copied the crate to `target/lg-w10-adv/mutant-allowed/` and made the allowed-bytes check also accept `@ : ; = + , ! $ & ( ) *`. Lib (41) and `tests/gateway.rs` (21) stayed green. My byte case failed: `assertion failed: "/!a"`. No CLI test or scenario puts any of those bytes in a base path. | Any `base_url` in an operator's deployment document. The real code refuses these bytes, so this is a test gap, not a defect. | CONFIRMED | introduced |

**5. Attacked, could not break**
- **Request-line injection:** the path is built from the wire constant, not the incoming request, and the base-path rule leaves no CR, LF, space or `%`.
- **Path escape:** no accepted base path joins to a dot segment (checked exhaustively up to 6 bytes).
- **`host` header:** it stays the authority alone, and the request head is sized after the path is joined, so it never reallocates.
- **Runpod targets:** `PodTarget` has no base path (the default `None`), and `relaying.rs` `authority()` drops the Runpod `/v1/` as before.
- **The count path:** the clamp's count goes through the same head builder, so it gets the joined path.
- **Credentials over http:** the cleartext rule doesn't change with or without a path.
- **Authority/path split edges:** `@` after `/`, `:` in the path, an IPv6 literal with a path, a bracket left open across `/` — all handled.
- **Wrappers:** the only wrapper, the conformance `Counted`, forwards the base path. The CLI and gateway crates have no wrapper.

**6. Written outside the worktree:** only the harness log on the header line. Scratch is in the tree under `target/lg-w10-adv/`.

```findings
[{"file": "crates/llm-gateway/src/relay.rs", "line": 208, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "a mutant admitting the URL delimiters @ : ; = + , ! $ & ( ) * into BasePath survives the unit's whole suite; only crates/llm-gateway/tests/adversary_w10_base_path.rs catches it"}]
```
