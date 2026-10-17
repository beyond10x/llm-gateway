---
format: aep.planning-md/3
id: review-result:adversary-w15-cross-wire-reasoning-state-pass-1
kind: review-result
status: active
title: Wave 2026-10-17-w02 adversary, story:cross-wire-reasoning-state, pass 1
relations:
- reviews: story:cross-wire-reasoning-state
revision: 1
---
## Pass

Wave 2026-10-17-w02 adversary, story:cross-wire-reasoning-state, pass 1, against `8103d8a` on
`impl/cross-wire-reasoning-state`. The adversary added three cases in
`crates/llm-gateway-cli/tests/adversary_w15_sealed_reasoning.rs` (committed `0072ac9`), all
green; with `translation.rs` the run reads 13 → 16 cases, 0 red.

| Case | Now |
|---|---|
| `adversary_w15_every_variant_of_a_seal_is_refused_and_the_genuine_one_still_binds` | green |
| `adversary_w15_a_same_wire_route_relays_a_seal_shaped_item_unchanged` | green |
| `adversary_w15_two_reasoning_blocks_are_sealed_and_bound_in_their_order` | green |

Attacked without a break: 26 forged or altered seals (padding, the standard base64 alphabet,
whitespace, prefix changes, doubled or cut parts, empty parts, non-string `encrypted_content`,
the chat route's null seal swapped in, 1 MiB parts, a seal sent to another route), none reaching
the upstream; constant-time tag check; scope bound by alias, upstream model and both wires; one
key per process, so a restart invalidates old seals; no panic on malformed input; same-wire
routes unchanged; nothing sealed reaches a log, event or metric.

## Findings

```findings
- file: spec/domains/gateway.yaml
  line: 69
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: acceptance 1 asks for the seal and its scope to be modelled in llm-gateway.gateway / llm-gateway.clients; they exist only as comments. Fixed in the same wave by modelling them as specification declarations.
```
