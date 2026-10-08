---
title: The single-owner gateway
sidebar_position: 1
description: The HTTP surface, who may call each route, the order requests are decided in, and the lifecycle.
lede: One owner holds one credential; the probes and two public routes answer without it, and everything else is authenticated before it is decoded.
source: docs/gateway.md, crates/llm-gateway/src/server.rs, crates/llm-gateway/tests/gateway.rs, crates/llm-gateway/tests/public_listing.rs, crates/llm-gateway/tests/dependency_boundary.rs
---

# The single-owner gateway

The gateway library, `b10x-llm-gateway` (`use llm_gateway`), admits exactly one owner. The
embedding resolves the owner's secret once, from whatever source it chose, and hands the gateway
an `OwnerToken`; the gateway is never given a credential *source*. The binary reads the secret
from the file `owner_secret_file` names. Clients send it as `Authorization: Bearer <secret>`.

## The surface

Every response is `cache-control: no-store` and `connection: close`: one request per connection.

| Method | Path | Credential | Answer |
| --- | --- | --- | --- |
| `GET`, `HEAD` | `/health` | none | `200 {"status":"live"}` while the process serves |
| `GET`, `HEAD` | `/ready` | none | `200 {"status":"ready"}`, or `503 {"status":"unready"}` before readiness and while draining |
| `GET`, `HEAD` | `/v1/models` | none | The relayed models in the OpenAI list shape |
| `GET`, `HEAD` | `/` | none | Setup instructions, chosen by `User-Agent` ([Set up a client](../guides/set-up-a-client.md)) |
| `GET`, `HEAD` | `/v1/routes` | owner | Every route in the inventory |
| `GET`, `HEAD` | `/v1/routes/{alias}` | owner | One route by alias |
| `GET`, `HEAD` | `/metrics` | owner | The counters as Prometheus text ([Scrape the counters](../guides/scrape-the-counters.md)) |
| `POST` | `/v1/chat/completions` | owner | The `chat` wire, relayed ([The relay](relay.md)) |
| `POST` | `/v1/responses` | owner | The `responses` wire, relayed |
| `POST` | `/v1/messages` | owner | The `messages` wire, relayed |

## The order a request is decided in

1. **A bounded head**, read under one deadline (`read_timeout`, 10 seconds by default) and at most
   8192 bytes. A head that is not HTTP/1.x, not UTF-8, or not whole in time is `request-malformed`.
2. **The probes**, `/health` and `/ready`, before anything else, so a liveness probe never needs
   the credential and never fails because the gateway is busy.
3. **Load shedding**: past 64 concurrent requests, `overloaded`. Shedding is cheaper than the work
   it sheds, so it comes before authentication.
4. **The public routes**, `GET` and `HEAD` of `/v1/models` and `/`. They read the relayed models
   and nothing else: never the route inventory, the target source or a credential, so neither wakes
   a pod or renders an endpoint, a digest or a byte of the owner secret.
5. **Authentication of everything else, before any further decoding.** An unauthenticated `POST`
   to an unknown path is `credential-absent`, not `path-unknown`, so the surface cannot be
   enumerated without the credential. The comparison is of SHA-256 digests in constant time, and a
   rejected credential is never echoed back. Every `401` carries `www-authenticate: Bearer`.
6. **The route**: inspection, the counters or a wire. An unknown path is `path-unknown`; a method
   the path does not take is `method-not-allowed` with an `allow` header.

[Refusals](../reference/refusals.md) lists every code with its status and message.

## Route inspection

`GET /v1/routes` renders a `RouteInventory`, an immutable snapshot of identifiers the composer
supplied: the protocol, provider, account, endpoint, model and binding revision of each target,
and the target's authentication and billing *kinds*. It has no field for an endpoint URL, a secret
reference or credential material, and it renders exactly the identifier bytes it was given. A
limit the source did not report, such as a context window, is absent from the answer; it never
becomes `0`. The binary builds the inventory from the deployment document: one route per model,
one target per wire, and the document's SHA-256 as `config_digest`.

## Lifecycle

| Step | What happens |
| --- | --- |
| Bind | The gateway listens at once and is **not ready**: a readiness probe cannot see it before the embedding has finished composing it. |
| Ready | `mark_ready` opens inspection and the wires. |
| Drain | `begin_drain` reports unready while it keeps serving, so a load balancer takes the gateway out of rotation without failing requests. A drain cannot be undone. |
| Stop | `shutdown` stops accepting, lets every accepted connection finish and reports `accepted` and `completed`, which are equal. |

The binary marks itself ready once it is composed, and on SIGINT or SIGTERM it drains, then
stops. For the probes and
inspection a stop takes at most twice `read_timeout`, however slowly a client sends or reads. A
relayed stream is waited for until it ends.

## What the library cannot do

Nothing in the gateway crate's build can resolve a credential, open a connection to an upstream or
create a billed resource: its whole dependency closure, as cargo resolves it, is the crate itself.
A test asserts that over the transitive closure. The relay speaks only over connections its
injected targets open, and the binary, `b10x-llm-gateway-cli`, carries the dependencies the
gateway crate must not have.

## Bounds

| Bound | Value |
| --- | --- |
| Owner credential | at most 4096 bytes |
| Shared secret | at least 32 bytes |
| Identifier (label) | at most 256 bytes |
| Targets per route | at most 64 |
| Routes | at most 4096 |
| `config_digest` | exactly 64 lowercase hexadecimal characters |
| Request head | at most 8192 bytes (a default the embedding may change) |
| Concurrent requests | at most 64 (a default the embedding may change) |
| Request body | at most 33554432 bytes (32 MiB) |

The gateway's tests check every bound in the contract against the source and flip each one at
exactly the published number. The contract itself is
[`docs/gateway.md`](https://github.com/beyond10x/llm-gateway/blob/main/docs/gateway.md).
