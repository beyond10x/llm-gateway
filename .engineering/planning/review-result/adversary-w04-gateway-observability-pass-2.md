---
format: aep.planning-md/3
id: review-result:adversary-w04-gateway-observability-pass-2
kind: review-result
status: active
title: Wave 2026-10-08-w04 adversary, story:gateway-observability, pass 2
relations:
- reviews: story:gateway-observability
revision: 1
---
## Pass

Wave 2026-10-08-w04 adversary, story:gateway-observability, pass 2, against the correction
`27f3a9b..b6a9140` on `impl/gateway-observability`. Nothing found; two probe cases ran green on
first run and were not committed.

| Probe | Result |
|---|---|
| `o1_o2_each_target_answer_shape_feeds_the_series_its_final_status_decides` (11 target answer shapes: 1xx then final, 302, 299, 204, body cut after a 200 or 500 head, closed with no byte, non-HTTP head, cold-start refusal) | `ok. 1 passed` |
| `o3_no_unreadable_rust_log_shape_adds_a_line_to_standard_error` (10 `RUST_LOG` values, empty, commas only, bracketed spans, non-UTF-8) | `ok. 1 passed` |

Package suites with both probes present: `cargo test --locked -p b10x-llm-gateway -p
b10x-llm-gateway-cli`, 226 passed, 0 failed.

Compared with llmgw `src/lib.rs:497-680` and `src/runpod.rs:317` at `048ebd8`: unanswered calls,
502/503/504, relayed 4xx/500, wire-not-served and cold-start counting match. Outside this
correction: llmgw follows redirects while this gateway relays a 3xx and counts it as a status
failure.

```findings
[]
```
