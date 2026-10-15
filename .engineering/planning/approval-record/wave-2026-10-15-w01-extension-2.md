---
format: aep.planning-md/3
id: approval-record:wave-2026-10-15-w01-extension-2
kind: approval-record
status: draft
title: Wave 2026-10-15-w01 extended with story:usage-details-pass-through
relations:
- decides: story:usage-details-pass-through
revision: 1
---
## Stop

One unit added to wave 2026-10-15-w01: `story:usage-details-pass-through` on
`impl/usage-details-pass-through`, forked from the wave branch after `story:max-tokens-clamp`
merged. The same pull request and release carry it.

Width: one unit at a time. It shares `tests/wire_relay.rs`, the `Relay` scenarios,
`contracts/suite.json` and `baseline.json` with the units before it; its `depends_on` edge
records it.

## Reason

non-interactive: a consumer asked to confirm that usage details, cached tokens included, reach the
client unchanged; it fits this release as fixture scenarios with no code change. Mapping them
into the usage record stays with `story:usage-records`, which waits on an llm release.

## Limits

Fixtures only: no pod, no provider call, no credential, no network beyond loopback, no spending.
