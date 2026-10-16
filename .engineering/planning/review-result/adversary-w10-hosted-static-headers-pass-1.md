---
format: aep.planning-md/3
id: review-result:adversary-w10-hosted-static-headers-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w01 adversary, story:hosted-static-headers, pass 1
relations:
- reviews: story:hosted-static-headers
revision: 1
---
unit: story:hosted-static-headers
verdict: red
cases: executed 391→396, red 2
origin: introduced 3, pre-existing 0, undecided 0
wrote-outside-worktree: none
needs-coordinator: no

Findings cover the working tree at `660a165` (branch `adv/hosted-static-headers`) plus my uncommitted test files.

**1. Diff stat (`git --no-pager diff --stat`)**
```
 crates/llm-gateway-cli/tests/hosted.rs | 54 ++++++++++++++++++++++++++++++++++
 1 file changed, 54 insertions(+)
```
Untracked: `crates/llm-gateway-cli/tests/adversary_w10_static_headers.rs` and `crates/llm-gateway/tests/adversary_w10_static_headers.rs`. All three are test files, and no implementation file was touched.

**2. Cases added (each was run alone first)**

| File::test | Asserts | Now |
|---|---|---|
| `crates/llm-gateway-cli/tests/adversary_w10_static_headers.rs::adversary_no_refusal_of_a_headers_table_quotes_a_value` | 8 malformed `headers` forms; no refusal quotes the value | **red** |
| `…cli/tests/adversary_w10_static_headers.rs::adversary_every_reserved_name_in_any_case_is_refused` | all 13 reserved names, plus `X-API-KEY` → `config:value` naming `hosted.remote.headers`, no value quoted | green |
| `…cli/tests/adversary_w10_static_headers.rs::adversary_every_token_character_and_both_value_ends_load` | every tchar in a name loads lowercased; a value with 0x20 inside and 0x7e at both ends loads | green |
| `crates/llm-gateway/tests/adversary_w10_static_headers.rs::adversary_a_static_header_cannot_repeat_the_credential_header_the_gateway_writes` | the head holds `x-api-key` once | **red** |
| `crates/llm-gateway-cli/tests/hosted.rs::adversary_static_headers_reach_the_output_clamps_count_too` | the `count_tokens` request carries the static headers in order (no test covered this) | green |

Red output on the first run of each:
```
refusals that quote a header value:
headers written as one string instead of a table: config:schema config:schema: line 10: invalid type: string "authorization: Bearer leaked-value-0123456789abcdef", expected a map
```
```
assertion `left == right` failed: the head repeats the credential header:
POST /v1/chat/completions HTTP/1.1
host: shadowing.invalid:8000
x-api-key: inline-not-the-credential
x-api-key: target-api-key-0123456789abcdef
...
  left: 2
 right: 1
```

**3. Suite run (after the cases existed)**

| Command (both with `RUSTUP_TOOLCHAIN=1.98.0 RUSTC_WRAPPER=sccache`) | Exit | Result |
|---|---|---|
| `cargo test --locked --no-fail-fast -p b10x-llm-gateway -p b10x-llm-gateway-cli` | 101 | 394 passed, 2 failed (the two red cases above) |
| Same command, with my 5 cases left out by `--skip` | 0 | 391 passed, 5 filtered out |

**4. Findings**

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| F1 | `crates/llm-gateway-cli/src/config.rs:86` (via `schema_refusal` at `:350`) | CONFIRMED / introduced | `headers = "authorization: Bearer <secret>"` is refused `config:schema`, and the message quotes the whole string | An operator writing the table as one string; the refusal goes to stderr at start. At base, `deny_unknown_fields` gave "unknown field `headers`", which quotes nothing (read with `git show 69905b5`, not run) |
| F2 | `crates/llm-gateway/src/relay.rs:1114` vs `docs/gateway.md:281` | INFEASIBLE / introduced | The library writes a `StaticHeader` equal to the credential header; the head carries `x-api-key` twice, against the doc's "cannot … repeat a header the gateway writes" | Nothing in the tree: `HostedEndpoint::with_headers` and the conformance runner refuse it first. Only a third-party embedding reaches it |
| F3 | `spec/domains/gateway.yaml:195` | CONFIRMED / introduced | The `CompositionRefusal` variants list gained no `StaticHeader*` variant, although the comment at `:184` and the story's "ESS first" section name `static-header:<rule>` | Only the spec. Base path has the same pre-existing gap |

Suggested fixes; I applied none of them:
- **F1:** report a wrong type for `headers` without serde's quoted literal, either with a custom deserializer or by redacting in `schema_refusal`.
- **F2:** make `request_head` skip a static header that equals the credential header, or reword the doc sentence to say this is the embedding's job.
- **F3:** add the variants.

**5. Attacked and not broken**
- Values holding CR LF, LF, NUL, DEL, a tab, `\e`, non-ASCII, a leading or trailing space, empty, or 1025 bytes: all refused, and none quotes the value.
- Names: 13 reserved in any case; the credential header in another case; a case-only repeat; an exact repeat; a dotted key; an inline table; a nested table; unicode.
- `RequestHead` sizing: the static headers are in `start` before `with_capacity`, so the head cannot reallocate.
- The conformance `Counted` wrapper forwards `headers`. `PodTarget` has none.
- Sort order is by lowercase name; the count to 32 and its boundaries hold.

**6. Paths written outside the worktree:** none. Logs are in the worktree at `target/lg-w10-adv/suite-after.log` and `target/lg-w10-adv/suite-deselected.log`. Lease released.

```findings
- file: crates/llm-gateway-cli/src/config.rs
  line: 86
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: headers declared as a string instead of a table is refused config:schema with serde's message, which quotes the whole string, so a credential written there is echoed to stderr
- file: crates/llm-gateway/src/relay.rs
  line: 1114
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: request_head writes a static header equal to the target's credential header, so the head repeats a header the gateway writes, contrary to docs/gateway.md:281; only the binary's with_headers prevents it
- file: spec/domains/gateway.yaml
  line: 195
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: CompositionRefusal gained no StaticHeader variants although the comment and the story name static-header:<rule> composition refusals
```
