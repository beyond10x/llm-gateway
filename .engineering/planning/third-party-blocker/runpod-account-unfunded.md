---
format: aep.planning-md/3
id: third-party-blocker:runpod-account-unfunded
kind: third-party-blocker
status: cleared
title: The Runpod account is unfunded, so no live pod run can happen
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-29T19:58:56Z", actor: "human:timo", revision: 3}
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

## Cleared by decision, 2026-10-29

The operator decided not to use Runpod (`approval-record:openai-hosted-qualification-no-runpod`).
The account stays unfunded; it was not funded and no pod was started. `story:client-qualification`
and `story:loom-qualification` were retargeted to OpenAI's hosted API and no longer wait on this
blocker: they now wait on the connectors OpenAI connector,
`upstream-blocker:connectors-openai-connector`, through
`story:hosted-credential-from-connectors`. Its `blocks` edges to both stories were removed before
the move.
