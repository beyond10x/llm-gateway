---
format: aep.planning-md/3
id: story:runpod-rest-crash-loop
kind: story
status: active
title: A crash-looping vLLM pod reached over REST is found and terminated
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T17:40:37Z", actor: "human:operator", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-26T17:40:38Z", actor: "human:operator", revision: 4}
---
## Outcome

A pod started through the REST transport whose vLLM container restarts again and again is found
crash-looping and terminated by the pool's crash-loop rule (`crash_restart_limit` restarts within
`crash_window_ms`), instead of staying `starting` until its startup deadline.

## Why

Measured on the owner's host, 2026-11-26, gateway on main with the REST transport (`api_key` on the
provider). Pod `b10x-llm-inkling-pod` on an RTX PRO 6000: the vLLM engine failed with
`CUDA error: no kernel image is available for execution on the device`, and the pod's log stream
(`GET /v2/pods/{id}/logs`) showed `start container for <image>: begin` at 15:29:38Z and 15:30:11Z,
each followed by `Engine core initialization failed`. The pod's `startedAt` stayed
`2026-11-26T15:22:24.33Z` throughout. `RestRunpod::container_started_at` returns the pod's
`startedAt`, so the pool saw no restart: the model stayed `starting` until the pod was deleted by
hand at 15:31:34Z, about 2 minutes after the second restart. A second pod on an H100 SXM
(4.39 USD/h) failed the same way and was still `starting` 30 s past its startup deadline when it
was deleted by hand.

## Acceptance

- Over the REST transport, a pod whose container starts more than `crash_restart_limit` times
  within `crash_window_ms` is terminated and its model answers the crash-loop refusal, in a test
  against the loopback fake Runpod API with a recorded log stream.
- The source of a container start is a field or endpoint of Runpod's published REST API v2
  document (`crates/llm-runpod/tests/fixture/runpod-openapi-v2.json`), and the openapi test holds
  the request to it.
- A pod past its startup deadline that never became ready is terminated by the next cleanup pass,
  with a test.
- No test calls the live API.
