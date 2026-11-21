---
format: aep.planning-md/3
id: upstream-blocker:llm-client-cache-marks
kind: upstream-blocker
status: cleared
title: No llm release carries a client's cache mark placement to the Messages and InvokeModel encoders
relations:
- blocks: story:bedrock-client-cache-marks
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-11-21T06:08:51Z", actor: "human:timo", revision: 3}
---
## What is missing

llm's neutral turn keeps only the lifetime of a Messages client's `cache_control` marks, and its
Messages encoder, which `llm_bedrock` uses for the `InvokeModel` body, places its own breakpoints
(a rolling one on the last markable block, one on the joined system block). The gateway cannot
send a client's marks where the client put them without llm carrying the placement: the body is
encoded and signed inside llm's Bedrock client.

## Cleared when

An llm release carries a client's mark placement through the neutral turn and its Messages and
`InvokeModel` encoders reproduce it. Record its tag here.

## Cleared 2026-11-21 by llm 0.24.0

beyond10x/llm 0.24.0 (https://github.com/beyond10x/llm/releases/tag/0.24.0, tag commit
08289a72fd69939bb3f0518474a213348f366377) carries `Hints::cache_marks` (`CacheMarks`): the
Messages ingress records where a client placed its marks, and the Messages and `InvokeModel`
encoders place exactly those and none of their own; a request without a mark keeps the system
and rolling breakpoints. Wave 2026-11-21-w04 moved the gateway's pin to it for
`story:bedrock-client-cache-marks`.
