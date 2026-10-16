---
format: aep.planning-md/3
id: third-party-blocker:runpod-account-unfunded
kind: third-party-blocker
status: open
title: The Runpod account is unfunded, so no live pod run can happen
relations:
- blocks: story:client-qualification
- blocks: story:loom-qualification
- blocks: story:spend-caps
revision: 1
---
## Blocks

The Runpod account has no funds, so no pod can start. Every story whose acceptance is a recorded
live run against a real pod waits on it: `story:client-qualification` (Claude Code and Codex
sessions) and `story:loom-qualification` (a Loom turn). `story:claude-code-wire` waits through its
`depends_on` edge to `story:client-qualification`.

## Clears when

The account owner funds the Runpod account and grants a spending ceiling for the live runs. Until
then no pod is started, no Runpod API is called and no credential is created.

## Not blocked

Stories proved against `EmulatedRunpod`, the `connectors-fixture` binary or loopback fixture pods
(recorded or scripted pod answers) do not wait on it: `story:pod-proxy-tls`,
`story:hosting-spec-declarations`, `story:hosted-endpoints`, `story:hosting-lifecycle-declared` and
`story:gateway-translation`.
