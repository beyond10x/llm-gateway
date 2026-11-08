---
format: aep.planning-md/3
id: story:pod-watchdog
kind: story
status: implemented
title: A watchdog process stops the gateway's pods when the gateway cannot
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/heartbeat.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/watchdog.rs
- confidence: cited
  path: deploy/systemd/llm-gateway-watchdog.service
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T16:18:56Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-04T16:18:56Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-04T16:18:56Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`b10x-llm-gateway watchdog` reads the same document, runs as its own process (an example systemd
user unit is in `deploy/systemd/`), polls Runpod itself every minute and terminates a pod the
gateway owns (name prefix `b10x-llm-` and the controller's owner tag) when the gateway's heartbeat
is stale, the pod is idle past its model's idle timeout, not ready past `not_ready_minutes`, past
`max_runtime_hours`, or the balance is under `min_balance_usd`. It never touches another pod.

## Acceptance

1. Each of the five rules terminates an owned pod against `EmulatedRunpod`, and a pod without
   the owner tag survives every rule.
2. The gateway writes the heartbeat file every 10 s: pid, time, per pod its last finished call
   and in-flight count.
