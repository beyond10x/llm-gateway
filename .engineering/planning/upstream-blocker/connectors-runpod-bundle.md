---
format: aep.planning-md/3
id: upstream-blocker:connectors-runpod-bundle
kind: upstream-blocker
status: cleared
title: No connectors release carries a Runpod OpenAPI bundle (design choice D1 = A)
relations:
- blocks: story:runpod-production-transport
- blocks: story:live-runpod-wiring
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-10-08T18:12:11Z", actor: "human:timo", revision: 4}
---
## What is missing

Design choice D1 = A (`decision-blocker:runpod-control-plane`) reaches the Runpod control plane
(create, list, terminate a pod) through beyond10x/connectors with a Runpod OpenAPI bundle. No
connectors release carries that bundle yet.

## Cleared when

A connectors release carries the Runpod OpenAPI bundle. Record its tag here.

## Cleared 2026-10-08 by connectors v0.36.0

beyond10x/connectors v0.36.0 (https://github.com/beyond10x/connectors/releases/tag/v0.36.0)
carries the Runpod pods bundle: `pod.create` (`CreatePod`, `POST /v1/pods`), `pods.list`
(`ListPods`, `GET /v1/pods`) and `pod.terminate` (`DeletePod`, `DELETE /v1/pods/{podId}`), from the
Runpod REST document pinned under connectors `adapters/runpod/upstream/` (its
`docs/catalog-runpod.md`). It selects no `GetPod`; `story:runpod-production-transport`
acceptance 1 reads a pod through `pods.list` with the `id` filter instead. Both writes are
required-approval mutations: each call carries a proof prepared and issued for its exact input.
