---
format: aep.planning-md/3
id: story:cross-wire-reasoning-state
kind: story
status: implemented
title: A translated route serves a client's next turn that replays the reasoning state the gateway gave it
relations:
- decomposes: epic:gateway-features
- informed_by: story:gateway-translation
- serves: vision:portable-model-inference
- depends_on: story:llm-0-7-0-pin
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: contracts/gateway/scenarios/translate-carried-reasoning-state-is-refused-without-asking-for-a-target.yaml
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: cited
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: cited
  path: crates/llm-gateway-cli/tests/translation.rs
- confidence: inferred
  path: crates/llm-gateway/src/error.rs
- confidence: cited
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/clients.yaml
- confidence: cited
  path: spec/domains/gateway.yaml
- confidence: cited
  path: website/docs/guides/translate-between-wires.md
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T10:09:02Z", actor: "human:timo", revision: 4}
- {from: "proposed", to: "active", at: "2026-10-17T10:09:03Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-10-17T19:00:39Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
## Outcome

On a translated (cross-wire) route, a client's next turn that replays the reasoning state the
gateway handed it is served, not refused `translation-opaque-state`. Codex's second turn through a
Responses-to-Chat or Responses-to-Messages route works; same-wire routes are unchanged.

## Why

The translated path refuses every carried opaque item (`crates/llm-gateway-cli/src/translate.rs:544`,
`story:gateway-translation` design 4), because binding unattributed state to whichever upstream
the request reaches would reinstate the laundering that `story:unattributed-opaque-state` closed.
Codex replays the reasoning items of its previous answer on every turn, so a cross-wire route
refuses its second turn. Reported by a downstream user of llm 0.7.0 (2026-10-17).

## Design to decide (spec first)

- **A, recommended: state the gateway minted, bound per route.** When an answer on a translated
  route carries upstream reasoning state, the gateway hands the client an opaque item it seals
  itself: the upstream's state plus the route and the upstream wire, authenticated with a key the
  gateway generates at start and never writes out. On the next turn it binds an item only if the
  seal verifies for the same route (llm's `TurnRequest::bind_unattributed` for that route's
  binding). Anything else stays `translation-opaque-state`. Nothing is laundered across routes or
  upstreams. After a restart old items fail to verify and are refused, which the docs say.
- **B: a per-route opt-in `drop_carried_reasoning = true`.** It drops carried opaque items on that
  route and counts each drop. It is simpler but loses the reasoning context, and it is a silent
  drop the current rule forbids.

## Acceptance (for A; revise if B is chosen)

1. Spec first: the seal, its scope (route and upstream wire) and the refusals are modelled in
   `llm-gateway.gateway` / `llm-gateway.clients`, valid with the newest `ess`; conformance
   scenarios cover a replayed item from the same route (bound), another route (refused), a forged
   or altered item (refused) and an item from before a restart (refused).
2. A fixture test runs Codex's recorded two-turn exchange through a Responses-to-Chat route and a
   Responses-to-Messages route: the second turn is relayed with the upstream state it was given.
3. No log line, event, metric or refusal carries the sealed bytes or the upstream's state.
4. `docs/gateway.md` (the opaque-state rule and refusal table), the translate guide and CHANGELOG
   say what is carried and when it is refused.
