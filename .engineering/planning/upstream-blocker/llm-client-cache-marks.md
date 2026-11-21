---
format: aep.planning-md/3
id: upstream-blocker:llm-client-cache-marks
kind: upstream-blocker
status: open
title: No llm release carries a client's cache mark placement to the Messages and InvokeModel encoders
relations:
- blocks: story:bedrock-client-cache-marks
revision: 1
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
