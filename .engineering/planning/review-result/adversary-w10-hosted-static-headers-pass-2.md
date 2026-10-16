---
format: aep.planning-md/3
id: review-result:adversary-w10-hosted-static-headers-pass-2
kind: review-result
status: active
title: Wave 2026-10-16-w01 adversary, story:hosted-static-headers, pass 2
relations:
- reviews: story:hosted-static-headers
revision: 1
---
```
unit: story:hosted-static-headers, covering a9ff8c3 (branch adv2/hosted-static-headers) plus my 2 uncommitted test files
verdict: NEEDS-CHANGE
cases: executed 411→416, red 3
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: none
needs-coordinator: none
```

The fix in 26bbe6c is incomplete. A TOML datetime written as `headers` loads and adds a junk header to every request. Writing `headers` as a number or boolean still quotes that value in the refusal.

**1. Diff stat.** `git diff --stat` is empty, so no tracked file changed. There are 2 untracked files, both tests:
- `crates/llm-gateway-cli/tests/adversary_w10_static_headers_pass_2.rs`
- `crates/llm-gateway/tests/adversary_w10_static_headers_pass_2.rs`

**2. Cases added (each run alone first; logs in `target/lg-w10-adv/pass2-*-alone.log`)**

| Case | Now |
|---|---|
| cli `adversary_pass2_headers_written_as_a_toml_datetime_is_refused` | **red** |
| cli `adversary_pass2_headers_written_as_a_non_string_scalar_quotes_no_value` | **red** |
| cli `adversary_pass2_other_shapes_are_refused_without_quoting` (11 shapes) | green |
| lib `adversary_pass2_a_static_header_named_like_the_credential_header_is_not_written_without_a_bearer` | **red** |
| lib `adversary_pass2_the_head_with_a_skipped_header_is_exactly_the_documented_bytes` | green |

Red output on the first run:
```
an offset datetime: accepted, every request is sent ["$__toml_private_datetime: 1979-05-27T07:32:00Z"]
a local datetime: accepted, every request is sent ["$__toml_private_datetime: 1979-05-27T07:32:00"]
a local date: accepted, every request is sent ["$__toml_private_datetime: 1979-05-27"]
a local time: accepted, every request is sent ["$__toml_private_datetime: 07:32:00"]
```
```
an integer: config:schema config:schema: line 10: invalid type: integer `4111111111111111`, expected a table of header names to string values
a float: config:schema config:schema: line 10: invalid type: floating point `31415926.5358979`, expected a table of header names to string values
a boolean: config:schema config:schema: line 10: invalid type: boolean `true`, expected a table of header names to string values
```
```
host: keyed.invalid:8000
x-api-key: static-not-the-credential
  left: 1
 right: 0
```

**3. Suite run, made after the cases existed** (`RUSTUP_TOOLCHAIN=1.98.0 RUSTC_WRAPPER=sccache cargo test --locked --no-fail-fast -p b10x-llm-gateway -p b10x-llm-gateway-cli`)

| Run | Exit | Result |
|---|---|---|
| With my cases | 101 | 413 passed, 3 failed (the 3 above) |
| Same, `-- --skip adversary_pass2_` | 0 | 411 passed |

**4. Findings**

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| F1 | `crates/llm-gateway-cli/src/config.rs:114` | NEEDS-CHANGE / introduced | toml 1.1.6 hands a datetime to `visit_map` as `{"$__toml_private_datetime": "<text>"}`. `$` and `_` are valid in header names, so writing `headers = 1979-05-27` loads and every request gets an extra header. It should be refused `config:schema`. | The operator's document, loaded through `parse` by the binary. The story's base 69905b5 had no `headers` field. |
| F2 | `crates/llm-gateway-cli/src/config.rs:194` (`Table` has no `visit_i64`/`visit_u64`/`visit_f64`/`visit_bool`) | CONFIRMED / introduced | When `headers` is an integer, float or boolean, serde's default refusal quotes the value. This is the same leak pass 1 fixed for strings. | The operator's document. The refusal goes to stderr at start. A secret that is a pure number is unlikely. |
| F3 | `crates/llm-gateway/src/relay.rs:1117` vs `docs/gateway.md:293` | INFEASIBLE / introduced | The doc says without condition that a static header named like the credential header "is not written". The code skips it only when the target also has a bearer. | Only a third-party embedding. The binary's `check_headers` refuses that name, and an `api-key` endpoint always has a key. |

Suggested fixes (none applied):
- **F1:** have `Table::visit_map` refuse the key `$__toml_private_datetime`, or deserialize through `toml::Value` and accept only `Value::Table`.
- **F2:** add the four scalar `visit_*` methods to `Table`, each returning an error that names only the shape.
- **F3:** reword the doc to "one named like the credential header it is sent", or skip the header whenever `credential_header` is set.

**5. Attacked and not broken**
- These were all refused with no value quoted:
  - an array of tables, and an array of inline tables
  - nested and doubly nested inline tables
  - an array, a datetime or an integer as a single value
  - a 1 MiB value and a 100,000-byte name
  - a multi-line string containing a header line
  - a quoted name containing CR LF
- Head bytes: with the skipped header in the middle and a 1024-byte value, the request matches the expected bytes exactly. Static headers come in order, the credential once, and `content-length` is right. The no-reallocation `debug_assert` did not fire.
- The skip compares lowercase to lowercase: `X-API-KEY` was skipped.

**6. Paths written outside the worktree:** none. Logs are inside it, in `target/lg-w10-adv/` (`pass2-cli-alone.log`, `pass2-lib-alone.log`, `pass2-suite-after.log`, `pass2-suite-deselected.log`). Lease `lg-w10-adv-headers-2` is released. Pass 1's record is in the wave tree's store (`lg-w10-wave`), not in this tree's store.

```findings
[
  {"file": "crates/llm-gateway-cli/src/config.rs", "line": 114, "category": "acceptance", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "headers written as a TOML datetime loads instead of being refused config:schema, because toml presents it as a one-entry map whose key $__toml_private_datetime is a valid token, so every request carries that header"},
  {"file": "crates/llm-gateway-cli/src/config.rs", "line": 194, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "header_table's Table visitor falls back to serde's default for integer, float and boolean, whose config:schema message quotes the value, contrary to every refusal quoting no value"},
  {"file": "crates/llm-gateway/src/relay.rs", "line": 1117, "category": "contract-drift", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "request_head writes a static header named like the credential header when the target has no bearer, while docs/gateway.md:293 says unconditionally that it is not written; only an embedding reaches it"}
]
```
