---
format: aep.planning-md/3
id: dependency-blocker:sessionspec-records-release
kind: dependency-blocker
status: cleared
title: sessionspec-records has no published tag to link
relations:
- blocks: story:interaction-record-store
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-11-26T04:20:22Z", actor: "human:operator", revision: 2, decided_on: {"recorded":{"verification":1}}}
---
## What is missing

The crate `sessionspec-records` exists on sessionspec's main (commit e36e6cc, 2026-11-26) with the
wire lift and the appender this story links, but the sessionspec repository has no remote, so no
git tag of it can be named in `Cargo.toml` and no CI run can fetch it.

## Cleared when

sessionspec is published on a remote and tags a release whose `sessionspec-records` exports
`wire::lift_call`, `wire::WireSession`, `wire::session_hash` and `append::Appender`.
