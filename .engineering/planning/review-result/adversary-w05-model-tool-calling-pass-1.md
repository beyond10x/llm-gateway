---
format: aep.planning-md/3
id: review-result:adversary-w05-model-tool-calling-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w05 adversary, story:model-tool-calling, pass 1
relations:
- reviews: story:model-tool-calling
revision: 1
---
```
unit: story:model-tool-calling, impl/model-tool-calling at 898e2bb (adversary commit 0fb5ebf on top)
verdict: NEEDS-CHANGE (1 red case; the main claim held under every probe)
cases: executed 231→236, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 5 paths (part 6)
needs-coordinator: no
```

**The main claim held.** A non-empty top-level `tools` sent to an `Absent` model is refused with 400 `tools-not-served`, and no target is acquired. This held on all three wires, behind auth and after `wire-not-served`. The only break is in config parsing: an inline-table spelling of `tool_calling` loads when it should be refused.

**1. Diff stat (898e2bb..HEAD).** Both paths are test files.
```
 crates/llm-gateway-cli/tests/adversary_w05_tool_calling.rs |  29 +++
 crates/llm-gateway/tests/adversary_w05_tool_calling.rs     | 204 +++++++
 2 files changed, 233 insertions(+)
```
Commit `0fb5ebf test: tool_calling as a TOML inline table is read as a variant, not refused (red)`

**2. Cases added (each run alone, before the suite)**

| Case | Asserts | Now |
|---|---|---|
| cli `adversary_w05_tool_calling_as_an_inline_table_is_refused_as_schema` | `tool_calling = { parsed = {} }` / `{ absent = {} }` is refused as `config:schema` | **red** |
| gw `…escaped_tools_key_is_still_refused_without_a_target` | `tools`, `tools`, whitespace and newlines, `[null]`, `[[]]`, `[{}]` followed by `[]`: all 400 `tools-not-served`, 0 acquisitions | green |
| gw `…chunked_tool_request_is_refused_without_a_target` | a chunked body gets the same refusal, 0 acquisitions | green |
| gw `…comes_after_auth_and_wire_not_served` | no credential gives 401 `credential-absent`; an undeclared wire gives `wire-not-served`; 0 acquisitions | green |
| gw `…a_parsed_model_asks_for_a_target_on_every_wire` | a `Parsed` model with tools on chat, responses and messages makes 3 acquisitions | green |

Red output, captured verbatim before the suite ran:
```
thread 'adversary_w05_tool_calling_as_an_inline_table_is_refused_as_schema' panicked at crates/llm-gateway-cli/tests/adversary_w05_tool_calling.rs:22:31:
tool_calling = { parsed = {} }: accepted as Parsed
test result: FAILED. 0 passed; 1 failed
```

**3. Suite run (after the cases existed)**
- `RUSTUP_TOOLCHAIN=1.98.0 CARGO_INCREMENTAL=0 cargo test --locked -p b10x-llm-gateway`: EXIT=0, 123 passed. That is 119 before plus my 4.
- The same command with `--no-fail-fast -p b10x-llm-gateway-cli`: EXIT=101, 112 passed and 1 failed, ``1 target failed: `-p b10x-llm-gateway-cli --test adversary_w05_tool_calling` ``. That is 112 before plus my 1.
- How I got 231: I subtracted my files from these same runs. I made no run with the tree as handed over.

**4. Findings**

| file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|
| `crates/llm-gateway-cli/src/config.rs:63` (`ToolCallingDocument`) | NEEDS-CHANGE / introduced | serde reads an externally tagged enum from a one-key TOML inline table, so `{ parsed = {} }` loads as `Parsed`. That contradicts `spec/domains/deployment.yaml:202-203` ("any other value is `config:schema`"). Red at `adversary_w05_tool_calling.rs:22`. | Nothing I found: an operator would have to write that spelling. That makes it a small finding. Suggested fix: deserialize from a string only (`try_from = "String"` or a `deserialize_str` visitor). The same pattern may affect `ThinkingDocument` and other document enums; I did not test those. |

**5. Attacked and held**
- **Acquisition site:** there is exactly one `acquire`, at `relay.rs:732`, and it runs after `served_model`. No other route or wire reaches a target earlier.
- **Scanner edge cases:**
  - Handled: escaped keys, duplicate top-level `tools`, `tools` nested below the top level, and non-array `tools` such as `null` or `{}`.
  - Equivalent mutant: dropping the `tools_opened` reset at `body.rs:219` changes nothing.
- **Config:** wrong case (`"Parsed"`), `true` and `""` are refused. Omitting the key gives `Absent` (the unit's own tests).
- **Composition:** the only CLI composition site, `relaying.rs:612`, carries the declaration through.
- **Not a defect against the claim:** vLLM also accepts `tools` together with `tool_choice: "none"` or a named tool choice without the parser flags, so the refusal may turn away a few requests a pod could have answered. The story specifies "non-empty `tools`" as the rule.

**6. Paths written outside the worktree**
- `~/.cache/llm-gateway-w05/tool-calling/adversary/gateway-cases.log`
- `~/.cache/llm-gateway-w05/tool-calling/adversary/cli-case.log`
- `~/.cache/llm-gateway-w05/tool-calling/adversary/suite-b10x-llm-gateway.log`
- `~/.cache/llm-gateway-w05/tool-calling/adversary/suite-b10x-llm-gateway-cli.log`
- `~/.cache/llm-gateway-w05/tool-calling/adversary/suite-cli-nff.log`

**7. Findings block**
```findings
- file: crates/llm-gateway-cli/src/config.rs
  line: 63
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: tool_calling = { parsed = {} } loads as Parsed instead of being refused config:schema as spec/domains/deployment.yaml:202-203 states, because the enum accepts TOML's inline-table form of an externally tagged variant
```
