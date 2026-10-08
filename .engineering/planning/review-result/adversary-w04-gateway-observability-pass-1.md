---
format: aep.planning-md/3
id: review-result:adversary-w04-gateway-observability-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w04 adversary, story:gateway-observability, pass 1
relations:
- reviews: story:gateway-observability
revision: 1
---
## Pass

Wave 2026-10-08-w04 adversary, story:gateway-observability, pass 1, against
`impl/gateway-observability` at `2ffdebc` (base `d57042a`). The adversary committed `27f3a9b`
(`crates/llm-gateway-cli/tests/adversary_w04_observability.rs`, one case), red:
`test result: FAILED. 0 passed; 1 failed`.

| Case | Red output |
|---|---|
| `o3_an_unreadable_rust_log_is_info_and_adds_no_line_to_standard_error` | `left: 3 right: 2`; the extra line is ``ignoring `llm_gateway_cli=loud`: error parsing level filter: ...`` |

Finding 2 was `undecided` because llmgw could not be read by the adversary. The coordinator read
llmgw `src/lib.rs` at `048ebd8`: `upstream_failures` counts transport failures only
(`:598-604`), and `route_upstream_status_failures` counts every non-success status the target
answered (`:622-629`).

Attacked without a break: unknown model names reach no label, record or log; label escaping;
exposition format, content type and HEAD length; monotone counters; no record for
unauthenticated, shed or head-parse-failed requests; `response_bytes` and duration; no secret in
any event; the dependency boundary; stderr lines byte-identical for unset, `info`, `debug`,
`warn`, `off`.

```findings
- file: crates/llm-gateway-cli/src/logging.rs
  line: 17
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: an unreadable RUST_LOG directive makes tracing-subscriber's parse_lossy print an unprefixed "ignoring ..." line to stderr, contradicting deployment.yaml's claim that at the default level the binary writes only its published lines
- file: crates/llm-gateway/src/metrics.rs
  line: 239
  category: contract-drift
  severity: warning
  verdict: INFEASIBLE
  origin: undecided
  message: llmgw_route_upstream_status_failures_total counts only gateway upstream-failed refusals, duplicating upstream_failures_total per route, while the llmgw name suggests relayed target failure statuses; llmgw src/lib.rs:193-210 could not be read to confirm
```
