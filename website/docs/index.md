---
slug: /
title: Overview
sidebar_label: Overview
sidebar_position: 1
description: What llm-gateway is, what it is not, and where it sits among its neighbours.
lede: llm-gateway is an authenticated HTTP gateway for one owner that relays three model wires to the targets its embedding hands out, with a hosting contract and a Runpod adapter for the pods behind them.
source: crates/ in the llm-gateway repository, docs/gateway.md, docs/hosting.md, README.md
---

# llm-gateway

llm-gateway is the **serving side** of llm. It is a Rust workspace with one binary,
`b10x-llm-gateway`, configured by one closed TOML document.

- The **gateway** admits one authenticated owner. It serves liveness and readiness probes, a
  public model listing, setup instructions for Codex, Claude Code and Loom, read-only route
  inspection and Prometheus counters. It relays the owner's Chat Completions, Responses and
  Messages requests to a model target and streams the answer back.
- The **hosting contract** gives every provisioned resource a provider-assigned identity, one
  lease at a time and stop obligations that only evidence discharges.
- The **Runpod adapter** runs a vLLM pod per model behind that contract, over an in-process
  emulator for tests and over the `connectors` CLI for the real control plane.

:::caution[Not deployed, not qualified]
Everything here is tested against in-process fakes, loopback sockets, the Runpod emulator and a
fixture of the `connectors` CLI. The shipped binary does **not** relay a request to a Runpod pod
yet: it refuses a document that declares a `connectors` provider until it can reach a pod's
`https` endpoint over verified TLS, so a relayed model is answered `target-unavailable`. No client
has run a live session against it. Read [Status](/docs/status) before you build on it.
:::

## What it is not

- **Not a model client.** The neutral turn, the protocol clients, credentials, routing and cost
  are llm's. llm-gateway answers llm's clients.
- **Not a translator.** It relays each wire as it arrives, rewriting only the top-level `model`
  (and an effort of `high`, which becomes `xhigh`, on the Messages wire). Protocol translation
  is planned, not built.
- **Not multi-tenant.** One owner, one credential. There are no accounts, quotas or API keys per
  user.
- **Not llmgw's replacement yet.** llmgw is the gateway this repository is meant to succeed. Its
  counters keep their names here, but it stays in service until every capability it has is covered
  and a cutover is qualified.

## Where it sits

| Neighbour | Relation |
| --- | --- |
| [LLM](https://beyond10x.github.io/llm/) ([GitHub](https://github.com/beyond10x/llm)) | The client side. `b10x-llm-runpod` takes Runpod's provider description, the pod's address template, from `b10x-llm-providers` at a release tag. |
| [ESS](https://beyond10x.github.io/ess/) ([GitHub](https://github.com/beyond10x/ess)) | Specifies llm-gateway: the system `llm-gateway` and its scenarios, run by the conformance suite in the gate. |
| [Connectors](https://beyond10x.github.io/ecosystem/connectors/) ([GitHub](https://github.com/beyond10x/connectors)) | The only way the Runpod transport reaches Runpod's control plane. It holds the Runpod API key, which the deployment document never names. |
| [Loom](https://beyond10x.github.io/loom/) ([GitHub](https://github.com/beyond10x/loom)) | An agent harness meant to reach a model on a pod through llm and this gateway. `GET /` already answers the llm catalog lines a Loom run needs; a recorded live run is planned. |

## What you can do with it today

- **Run the binary** on a closed TOML document and read its probes, its model listing and its
  routes. [Getting started](getting-started.md)
- **Ask the gateway how to configure a client**: Codex, Claude Code or Loom.
  [Set up a client](guides/set-up-a-client.md)
- **Scrape llmgw's 13 counters** from `GET /metrics`. [Scrape the counters](guides/scrape-the-counters.md)
- **Embed the gateway library** in your own binary with your own target pool: the relay, the
  bounds and every refusal are the library's. [The relay](concepts/relay.md)
- **Run the gate**: Rust, the specification, conformance and these pages.
  [Run the checks](guides/run-the-checks.md)

## Where to go next

1. [Getting started](getting-started.md): build the binary and ask it something.
2. [The single-owner gateway](concepts/single-owner-gateway.md): the surface and what it refuses.
3. [The deployment document](reference/deployment-document.md): every key the binary reads.
