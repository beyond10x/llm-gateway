---
format: aep.planning-md/3
id: story:runpod-rest-host
kind: story
status: active
title: The Runpod REST client calls a host that resolves, and says which one it could not reach
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T05:48:10Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T05:48:10Z", actor: "human:operator", revision: 3}
---
## Outcome

The gateway's Runpod REST client calls a host that resolves and answers, and a wake of a model whose
provider it cannot reach answers `target-unavailable` naming the provider.

## Why

Measured on the owner's host, 2026-11-26, with 0.25.0 serving a managed model on `[providers.runpod]`
with a keychain `api_key`: the binary started, logged `Runpod REST API unreachable; its models
answer target-unavailable provider="runpod"` and `runpod account read failed`, and
`getent hosts rest.runpod.io` returned no address while `api.runpod.io` resolved. The client's base
`https://rest.runpod.io/v1` is compiled into the binary (deployment document, `api_key`). 0.24.0
logged the same account read failure on the same host before the switch. No pod was started.

## Acceptance

1. The REST client calls `https://api.runpod.io/v2` for pod list, create and terminate (the calls the client makes today), and
   for pod billing, in the v2 shapes named under "Documented route"; the pull request cites those
   pages. The v2 OpenAPI document (https://docs.runpod.io/api-reference-v2/openapi.json) is
   committed as a recorded fixture with its digest, and the fake Runpod's answers are checked
   against it. No test makes a network request.
2. The pod list follows pagination; a test with two pages sees every pod.
3. The balance comes from `GET /v2/billing/balance`; on 403 or 404 the client reads GraphQL
   `myself { clientBalance }` at `https://api.runpod.io/graphql` as today. A failed pod list no
   longer hides the balance: the dashboard shows the balance with the pods marked unread.
4. A test against a fake Runpod whose name does not resolve: a wake of a managed model answers
   `target-unavailable`, its message naming the provider, and starts nothing.
5. The startup warning and the account-read warning name the host and the HTTP status or the
   resolver error, never the key.
6. No pod is started on the owner's account to prove it; the host check after install is the
   account read (balance and pod list) only.

## Documented route

The client moves to Runpod's REST API v2, base `https://api.runpod.io/v2`, as documented on
2026-11-26:

- https://docs.runpod.io/api-reference-v2/migrate-from-v1.md: "REST API v1 is deprecated and will
  be retired on November 15, 2026"; v1 base `https://rest.runpod.io/v1`, v2 base
  `https://api.runpod.io/v2`; authentication unchanged (`Authorization: Bearer <key>`).
- https://docs.runpod.io/api-reference-v2/pods/list-pods.md: `GET /v2/pods`, answer
  `{"pods":[...]}`, paginated (follow `pagination.nextCursor` while `pagination.hasNextPage`),
  filters in the query ignored.
- https://docs.runpod.io/api-reference-v2/pods/create-a-pod.md: nested body (`name`, `image`
  required, `gpu: {id, count}`, exactly one of `gpu` or `cpu`, ports listed explicitly, one GPU
  type per pod).
- https://docs.runpod.io/api-reference-v2/pods/trigger-a-pod-state-transition.md:
  `POST /v2/pods/{id}/action` with `{"action":"start|stop|restart|terminate"}`; 409 when the
  action is not valid for the pod's status; scope `pods:write`.
- https://docs.runpod.io/api-reference-v2/pods/terminate-a-pod.md: `DELETE /v2/pods/{id}`.
- https://docs.runpod.io/api-reference-v2/billing/get-the-account-balance.md:
  `GET /v2/billing/balance`, answer `balanceAmount` (USD) and `billingMode`
  (`PREPAID`/`POSTPAID`), scope `billing:read`, 404 for an organization without a billing account.
  The migration page still says to read the balance through GraphQL (`myself { clientBalance }`);
  the two pages disagree.
- https://docs.runpod.io/api-reference-v2/billing/get-pod-billing-history.md:
  `/billing/pods` becomes `/v2/billing/pods`.

The page the owner posted, https://agents.runpod.io/ (also docs.runpod.io/agent-setup.md), names
Runpod's hosted MCP server, not a REST base; the gateway does not use MCP.
