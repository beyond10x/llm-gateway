---
format: aep.planning-md/3
id: dependency-blocker:llm-0-7-0-ingress
kind: dependency-blocker
status: cleared
title: llm 0.6.0's ingress refuses the fields Claude Code and Codex send
relations:
- blocks: story:client-profiles-translated
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-17T10:09:13Z", actor: "human:timo", revision: 2}
---
## What is missing

llm 0.6.0's Messages and Responses ingress refuses the fields Claude Code and Codex send
(`crates/llm-gateway-cli/tests/translation.rs`, the known-limit tests). Until a release of llm
carries, translates or explicitly refuses those fields, a translated model cannot be offered to
either client honestly.

## Cleared when

llm 0.7.0, which widens those ingress subsets, is released and this repository's pin moves to it.
