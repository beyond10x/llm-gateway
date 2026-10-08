---
format: aep.planning-md/3
id: upstream-blocker:connectors-runpod-bundle
kind: upstream-blocker
status: open
title: No connectors release carries a Runpod OpenAPI bundle (design choice D1 = A)
relations:
- blocks: story:runpod-production-transport
- blocks: story:live-runpod-wiring
revision: 2
---
## What is missing

Design choice D1 = A (`decision-blocker:runpod-control-plane`) reaches the Runpod control plane
(create, list, terminate a pod) through beyond10x/connectors with a Runpod OpenAPI bundle. No
connectors release carries that bundle yet.

## Cleared when

A connectors release carries the Runpod OpenAPI bundle. Record its tag here.
