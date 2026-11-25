---
format: aep.planning-md/3
id: story:cost-per-call
kind: story
status: draft
title: A model may declare a price, and a call record names its cost
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:call-id-and-usage-record
revision: 1
---
## Outcome

A model may declare what its tokens cost, and each call's record then names the call's cost.

## Why

The operator's goal of 2026-11-25 names per-call usage, tokens and cost, that a dispatcher can
book. Today the gateway prices no call: a route's `price` in the routes document is refused
(`website/docs/reference/deployment-document.md`, "`routes`").

## Acceptance

1. A hosted model, a pod model and a route of the routes document may each declare a price per
   million tokens: input, output, cache read and cache creation, in USD. The routes document's own
   `price` is read as llm writes it.
2. The call record of `story:call-id-and-usage-record` carries `cost_usd` for a call whose model
   declares a price and whose answer reported tokens. A model without a price, and a call without
   reported tokens, have no `cost_usd`: never `0`, never an estimate.
3. The cost is computed by llm's own pricing code, or by a rule published in `docs/gateway.md`
   with a test per token class.
4. The specification, scenarios, `docs/gateway.md`, the deployment reference and `CHANGELOG.md`
   carry the keys and the field.

## Open

The consumer of 2026-11-25, the dispatcher's design, prices a call itself from the tokens and a
price on its own route. This story is built when the operator asks for the gateway's own cost.

## Depends on

`story:call-id-and-usage-record`.
