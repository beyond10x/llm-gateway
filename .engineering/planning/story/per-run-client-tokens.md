---
format: aep.planning-md/3
id: story:per-run-client-tokens
kind: story
status: draft
title: The gateway accepts short-lived per-run client tokens
summary: 'Glovebox wave 6 (SM-D200): a verifier behind the OwnerVerifier seam admits model calls with a short-lived per-run token, so a box holds no long-lived model credential.'
tags:
- auth
- glovebox
relations:
- informed_by: epic:gateway-features
revision: 1
---
# Story: the gateway accepts short-lived per-run client tokens

Part of Glovebox (atlas `initiative:glovebox`, `docs/design/glovebox.md`), wave 6.

## Why

- Glovebox rule SM-D200: "An optional second path: `llm-gateway` holds the model credential and
  the box holds only a short-lived per-run token. The manifest chooses per session"
  (`atlas/docs/design/glovebox.md:56-57`). Wave 6 assigns "per-run client tokens through
  `OwnerVerifier`" to this repository (`atlas/docs/design/glovebox.md:95`).
- Today a model call carries the one long-lived owner secret: "The owner secret must be one
  printable token of 32 to 4096 bytes … Clients send it as `Authorization: Bearer <secret>`"
  (`README.md:142-144`). Under `ingress_auth = "none"` model calls take no credential at all
  (`README.md:130-135`). Neither gives a box a credential that is scoped to one run and expires.
- The seam exists: `OwnerVerifier::verify` decides on presented material
  (`crates/llm-gateway/src/auth.rs:267-270`), and capability matrix row K2 names it as
  llm-gateway's seam for identity-backed auth (`docs/llmgw-capability-matrix.md:102`). Its verdict
  has two values, `Owner` and `Rejected` (`crates/llm-gateway/src/auth.rs:232-237`), and an
  `Owner` verdict also opens the counters and the route inventory (`README.md:132-133`), so a
  per-run token cannot simply be a second owner.

## What changes

1. A design first (it adds an authentication path): the token format and issuer (who mints a
   per-run token and with which key), its lifetime and binding (run id, allowed model aliases,
   optionally a token or request budget), how the gateway verifies it offline, and a verdict that
   admits model calls only, never the owner's inspection surface. The ESS specification gains the
   nouns before code (`spec/`).
2. A verifier behind the `OwnerVerifier` seam, or a sibling seam the design names, composed
   beside the existing shared-secret verifier. The owner secret keeps working unchanged.
3. Configuration names the trusted issuer key; a gateway without it refuses every per-run token
   by name, so the path is optional.
4. Each relayed call's log line and, once `story:usage-records` lands, its usage record name the
   run the token was minted for.

## Acceptance

- The design is accepted and the specification validates with the new nouns.
- A model call with a valid per-run token is relayed; the same token is refused by name after it
  expires, for a model alias it does not allow, and on the counters and route inventory.
- A token signed by an unknown key, a malformed token and a token for another audience are each
  refused by name, and none is logged.
- With no issuer configured, every per-run token is refused and the owner secret path is
  unchanged; the existing suite passes.

## Blocked on / Depends on

- Nothing upstream; Atlas lists wave 6 as waiting on nothing (`atlas/docs/design/glovebox.md:95`).
- Mantle's `story:llm-gateway-model-mode` depends on this story.
- Open, from Atlas: whether the gateway relays a Claude Code conversation on the Anthropic wire
  across turns without translation is unverified (`atlas/docs/design/glovebox.md:102-104`); the
  Mantle mode needs it for Claude Code, this story does not.
