---
format: aep.planning-md/3
id: review-result:critic-acceptance-w10-round-1
kind: review-result
status: active
title: Acceptance critic, wave 2026-10-16-w01 new stories, round 1
relations:
- reviews: story:hosted-base-path
- reviews: story:hosted-static-headers
- reviews: story:ingress-auth-optional
revision: 1
---
approve

What I read: 4 artifacts (the 3 stories plus `epic:gateway-features` for context), with `aep plan artifact show <id>`. I also checked the tree with `ls` and `git grep`: the `hosted-*` and credential scenarios exist, and `listening on` is in `serve.rs:215`.

- **Acceptance:** each story's Acceptance section is a set of numbered cases. Each case names an observable (a relayed path, a head read by a CLI test, a `config:value` refusal, a scenario, a binary test) and its before state.
- **Transitions:** the states before and after differ in each case. Today the three stories behave as follows:
  - **`story:hosted-base-path`:** the upstream path equals the ingress path, so `/api/v1/...` is unreachable.
  - **`story:hosted-static-headers`:** no head carries a declared header.
  - **`story:ingress-auth-optional`:** every wire call authenticates first.

What I could not establish:
- Whether the `contracts/gateway/scenarios/hosted-*base*`, `hosted-*header*` and `ingress-*` scenarios exist. They are the stories' own new work, so none is expected yet.
- Out of my lane, did not set the verdict: `story:ingress-auth-optional` says it "runs after" the two hosted stories but has no `depends_on` relation to them. That is for `plan-critic-parallel-safety` and `plan-critic-design`.

```findings
[]
```
