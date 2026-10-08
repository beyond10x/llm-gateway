---
format: aep.planning-md/3
id: review-result:adversary-w02-provider-key-files-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w02 adversary, story:provider-key-files, pass 1
relations:
- reviews: story:provider-key-files
revision: 1
---
## Report

Adversary pass 1 against `story:provider-key-files`, commit 8b7587e on `impl/provider-key-files`
(base 0856b05). New file `crates/llm-gateway-cli/tests/adversary_w02.rs`: 5 passed, 3 failed. No
key bytes reached standard error, standard output, a response, `Debug` or a refusal message.

```findings
- file: crates/llm-gateway-cli/src/serve.rs
  line: 125
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: the vllm_keys doc says every refusal names the model, but the trusted-file rule refusals (symlink, unreadable, not-regular, owner, mode, size, not-utf8) name only the path
- file: docs/llmgw-capability-matrix.md
  line: 41
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: B9 moved from partial to covered, but the totals line still says 57 covered and 12 partial; the rows count 58 and 11
- file: docs/llmgw-capability-matrix.md
  line: 170
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the inserted vLLM key code moved serve.rs, so the D2 and C1 citations (132-151, 160-170) and spec/domains/upstream.yaml:88 (serve.rs:91, 112) now land on the new code instead of wait_for_stop, start and the per-wire targets
```
