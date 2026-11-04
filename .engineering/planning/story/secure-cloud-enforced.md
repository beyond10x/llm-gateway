---
format: aep.planning-md/3
id: story:secure-cloud-enforced
kind: story
status: implemented
title: Every pod the gateway creates is a Secure Cloud, on-demand pod of an allowed image
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-runpod/src/config.rs
- confidence: cited
  path: crates/llm-runpod/src/provider.rs
- confidence: cited
  path: spec/domains/deployment.yaml
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-04T16:18:55Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-11-04T16:18:55Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-11-04T16:18:55Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`COMMUNITY` is gone from the document and the adapter. Every create sends `cloudType: SECURE` and
`interruptible: false`; after a create the adapter reads the pod's placement
(`GET /pods/<id>?includeMachine=true`) and terminates it at once unless `machine.secureCloud` is
`true` and `interruptible` is not `true` (an absent `interruptible` counts as false, as Runpod
omits it). Only images with the prefix `vllm/vllm-openai:` are allowed.

## Acceptance

1. `cloud_type = "COMMUNITY"` is `config:schema`; an image outside the prefix is `config:value`.
2. Adversary tests: an emulated Community pod, an interruptible pod and a pod whose placement
   cannot be read are each terminated at once and never served.
