---
title: Hosting
sidebar_position: 3
description: The owned-resource hosting contract, the Runpod adapter behind it, and how far the binary uses either today.
lede: A provisioned resource is known by the identity its provider assigned, held under one lease, and stopped only when evidence says so.
source: docs/hosting.md, crates/llm-provision, crates/llm-runpod, crates/llm-provision/tests/hosting.rs, crates/llm-runpod/tests/runpod.rs, crates/llm-runpod/tests/transport.rs
---

# Hosting

Model targets run on billed compute. `b10x-llm-provision` is the lifecycle every hosting adapter
is held to, and `b10x-llm-runpod` is the first adapter behind it. Neither the contract nor the
gateway library opens a socket to a cloud API.

:::caution[Planned: the binary starts no pod yet]
The Runpod pool and its production transport are built and tested against `EmulatedRunpod` and a
fixture of the `connectors` CLI. The shipped binary refuses a document that declares a
`connectors` provider, because a pod is reached at its `https` proxy URL and the binary has no TLS
client for it yet. Until that lands, the binary creates no pod and relays to none. The transport
has not run against the real `connectors` CLI.
:::

## Three rules of the contract

| Rule | What it prevents |
| --- | --- |
| **A name is not an identity.** A resource is `(provider, account, name, incarnation)`, the incarnation assigned by the provider at creation. | Adopting, billing against or stopping a later resource that reused a name. |
| **Requested is not observed.** What an operator asked for and what the provider reported are different types; an unreported readiness, address or model is unknown, never the requested value. | Treating a pod as ready, or as serving the requested model, because nobody said otherwise. |
| **Only evidence discharges a stop obligation.** Absence from a complete listing, a provider-reported termination, an accepted stop or an external confirmation closes it; a disconnect, an expired lease or a restart does not. | Paying for compute that everyone believes is stopped. |

The `FakeProvider` in the same crate demonstrates the contract in process. Listing and validation
allocate nothing, and a create whose answer was lost keeps its obligation open and is never
retried blindly.

## The Runpod adapter

`b10x-llm-runpod` runs one vLLM pod per model behind the contract. Every control-plane call goes
through the `RunpodTransport` trait, which has two implementations:

| Transport | Used by | Reaches |
| --- | --- | --- |
| `EmulatedRunpod` | the tests | an in-process control plane |
| `ConnectorsRunpod` | the production composition | Runpod, only through the [Connectors](https://beyond10x.github.io/ecosystem/connectors/) ([GitHub](https://github.com/beyond10x/connectors)) CLI and its Runpod bundle |

The pool it composes:

- starts **one pod per model** however many first requests arrive together, and holds the later
  ones on it;
- tries the declared GPU types **in order**, but ends the attempt when a create's answer is lost,
  because Runpod takes no idempotency key and a second create could pay twice;
- **terminates** a pod that crash-loops, refuses its vLLM key or misses its startup deadline, and
  replaces it on the next request;
- **reaps** a pod idle past `idle_timeout_minutes`, never below its measured cold start and never
  with a request in flight;
- **sweeps** pods carrying this controller's owner tag that no record holds. A pod whose name
  starts with `llmgw-` is never listed, adopted, swept or terminated.

`ConnectorsRunpod` creates, lists and terminates pods with the `connectors` operations
`pod.create`, `pods.list` and `pod.terminate`. Each write is prepared and issued for its exact
input with an approval proof and sent once. The Runpod API key stays in the connectors keyring:
the deployment document never names it and refuses `runpod_api_key_file`. A pod is given its
model's vLLM key as a Runpod secret reference, and the gateway sends the same key to the pod as a
bearer.

The pod's address comes from llm's Runpod provider description
([LLM](https://beyond10x.github.io/llm/), [GitHub](https://github.com/beyond10x/llm)):
`https://<pod id>-8000.proxy.runpod.net/v1/`.

## Modal

`b10x-llm-modal` exists as a crate and exports nothing yet. A Modal adapter is planned.

The full contract, with every lifecycle state and the evidence each transition records, is
[`docs/hosting.md`](https://github.com/beyond10x/llm-gateway/blob/main/docs/hosting.md).
