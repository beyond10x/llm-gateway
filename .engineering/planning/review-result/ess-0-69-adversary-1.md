---
format: aep.planning-md/3
id: review-result:ess-0-69-adversary-1
kind: review-result
status: active
title: Adversary, story:ess-0-69, pass 1
relations:
- reviews: story:ess-0-69
revision: 1
---
needs-change

Adversary pass over the ESS 0.69.2 and llm 0.15.0 move (working tree on a0d9cb4), read-only:
the brief kept cargo test out of the tree while the gate ran.

**Attacked, not broken.** No path runs an `aws-process` credential command: `llm-models` resolves
with no feature and `llm-bedrock` without `process`, and the gateway refuses every Bedrock
endpoint of a routes document (`crates/llm-gateway-cli/src/routes.rs:222`). No client header but
`anthropic-beta` reaches llm's decoder. Every new llm 0.14.0 routes key is refused by llm or by the
Bedrock check. No `aws-lc`, `keyring` or `llm-cli` enters `Cargo.lock`.

**Fixed in the same story.** The proposed case is
`crates/llm-gateway-cli/tests/adversary_ess_0_69_opencode.rs`, red against `docs/gateway.md` at
a0d9cb4 (the header is absent there), green after the fixes below.

```findings
- file: docs/gateway.md
  line: 581
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "llm 0.15.0 sends x-opencode-session, a new value per request, to an OpenCode Go endpoint the gateway accepts, and the upstream header list omits it. Fixed: the contract names the header, its form and that it is per translated request."
- file: crates/llm-gateway-cli/src/routes.rs
  line: 582
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: "the comment saying relayed messages endpoints get what llm's client sends is false for OpenCode Go. Fixed: the comment says a relayed request carries no session."
- file: docs/gateway.md
  line: 444
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: "the contract and spec comments still named llm 0.13.0 in 16 places. Fixed: they name 0.15.0."
```
