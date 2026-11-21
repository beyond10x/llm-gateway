---
format: aep.planning-md/3
id: story:gateway-sa-token-ingress
kind: story
status: active
title: A Kubernetes service-account token admits a model call, checked by TokenReview
summary: ingress_auth = "service-account-token" admits a caller whose bearer the cluster's TokenReview authenticates for the configured audience as an allow-listed service account, beside the owner's bearer; every failure refuses
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T01:53:08Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-21T01:53:08Z", actor: "human:timo", revision: 4}
---
## Source

The operator, 2026-11-21, asked whether to file the gap as an issue: "And the LLM gateway one, just
implement it straight away on your own. All changes need also to be submitted with the Atlas bot
token from Beyond 10x. Not spot token, bot token."

A deployment whose agents reach the gateway through a loom boundary relay cannot reach it at all:
the relay puts the pod's projected service-account token, with a declared audience, in place of
any bearer (beyond10x/loom 0.57.2 `crates/loom-operator/src/render.rs:88-96`), a `bearer` gateway
refuses that token, and loom's admission refuses an owner-token Secret beside a boundary.

## Outcome

`ingress_auth = "service-account-token"` admits a model call whose bearer the cluster's TokenReview
API authenticates for the configured audience as a service account on the configured allow-list.
The owner's bearer still admits it when `owner_secret_file` is set, so one deployment serves a
backend holding the owner secret and boundary agents with projected tokens. Every failure of the
review refuses; positive reviews are cached by a hash of the token, bounded in size and time; the
admitted principal is in the call's usage record. A document that does not name the mode behaves
as before.

## Acceptance

1. Under `service-account-token`, a model call whose token the review authenticates for the
   audience as an allow-listed account is relayed, and its usage record names the account.
2. Another audience, an account off the list, an unauthenticated or expired token is
   `credential-rejected`; an unreachable review, a non-success answer, a malformed answer and a
   timeout are `credential-review-unavailable`; an empty allow-list refuses the start
   `config:value`. No token, part of one or review body reaches a log line, an error or a label.
3. A second call with the same token inside the cache window sends no second review; one after
   the entry expires does. No entry outlives the token's own expiry or 60 seconds; a rejection is
   kept 3 seconds at most; at most 8 reviews run at once.
4. A `bearer` document behaves as before: a token the reviewer would admit is still
   `credential-rejected` and no review is sent.
5. The gateway crate links nothing new (`dependency_boundary.rs`); the review runs in the binary
   over its existing rustls-on-ring client; `Cargo.lock` gains no crate.
6. `docs/gateway.md`, the deployment document reference, a Kubernetes guide (audience, RBAC,
   NetworkPolicy), the specification, scenarios, README and `CHANGELOG.md` say so in the same
   change.
