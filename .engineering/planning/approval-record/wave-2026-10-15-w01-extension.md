---
format: aep.planning-md/3
id: approval-record:wave-2026-10-15-w01-extension
kind: approval-record
status: active
title: Wave 2026-10-15-w01 extended with two units, no operator present
tags:
- non-interactive
relations:
- decides: story:effort-default
- decides: story:max-tokens-clamp
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-15T12:59:08Z", actor: "human:timo", revision: 2}
---
## Stop

Two units added to wave 2026-10-15-w01 after a consumer's live test of 0.7.0:
`story:effort-default` on `impl/effort-default`, then `story:max-tokens-clamp` on
`impl/max-tokens-clamp`, each forked from the wave branch after the unit before it merged
(`story:claude-code-wire` first). The same pull request and release carry all three.

Width: one unit at a time. The three stories share `crates/llm-gateway/src/body.rs`, `relay.rs`,
`tests/wire_relay.rs`, the `Relay` command and scenarios, "The relay" in `docs/gateway.md` and
`CHANGELOG.md`; the `depends_on` edges between them record it.

## Reason

non-interactive: this repository is worked by a controller session with no operator turn; the
operator put both findings of the live test into this release as stories, specification first,
tested against authored fixtures.

## Panel

Two rounds of the four plan critics: `review-result:critic-*-w08-round-1` and
`review-result:critic-*-w08-round-2`. Round 1: seven findings, all fixed. Round 2: design, scope
and parallel safety approve; acceptance raised one finding (which effort key counts as carried),
fixed in the body after the round with no third round run.

## Limits

Fixtures only: no pod, no Runpod or connectors live call, no credential, no network beyond
loopback, no spending.
