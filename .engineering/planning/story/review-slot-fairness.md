---
format: aep.planning-md/3
id: story:review-slot-fairness
kind: story
status: draft
title: A flood of never-admitted tokens cannot take every review slot
summary: per-source fairness for the service-account review slots, or a reserved share for tokens whose hash was admitted before, so a listed caller is reviewed during a flood
relations:
- decomposes: epic:gateway-features
- derived_from: review-result:security-sa-token-pass-1
revision: 1
---
## Source

Finding 3 of review-result:security-sa-token-pass-1 (the independent security review of pull
request 54): a caller with no valid token who knows the configured audience can send distinct
JWT-shaped tokens naming it and a future `exp`. Each passes the refuse-only pre-filter, takes one
of the 8 shared review slots and costs an API server call; while the slots are taken, a listed
caller whose admission is not cached is `credential-review-unavailable` (503). The coordinator
accepted the behaviour behind the guide's NetworkPolicy for the release that ships
`ingress_auth = "service-account-token"`, pinned by
`conformance_a_flood_naming_the_audience_holds_eight_reviews_and_refuses_only_uncached_callers_with_503`
in `crates/llm-gateway-cli/tests/token_review_conformance.rs`.

## Outcome

A flood of tokens that were never admitted cannot take every review slot: either slots are shared
fairly per source (the caller's address, which the gateway crate would hand to the review), or a
share of the slots is reserved for tokens whose hash was admitted before, so a listed caller whose
cached admission expired is reviewed again during a flood.

## Acceptance

1. During a flood of distinct forged tokens naming the audience, a listed caller whose admission
   expired from the cache is reviewed and admitted.
2. The flood still never has more than `concurrent-reviews` reviews in flight at the API server.
3. Nothing read from a token's unverified claims admits it or names its principal.

## Not in this story

Accepted for implementation only by the operator; drafted from a review finding.
