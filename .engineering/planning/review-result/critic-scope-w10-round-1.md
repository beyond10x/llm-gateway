---
format: aep.planning-md/3
id: review-result:critic-scope-w10-round-1
kind: review-result
status: active
title: Scope critic, wave 2026-10-16-w01 new stories, round 1
relations:
- reviews: story:hosted-base-path
- reviews: story:hosted-static-headers
- reviews: story:ingress-auth-optional
revision: 1
---
approve

What I read: 4 artifacts (`epic:gateway-features`, `story:hosted-base-path`, `story:hosted-static-headers`, `story:ingress-auth-optional`), using `aep plan artifact show` on each and `aep plan artifact graph`. I extracted 13 promises from the request and traced all 13 to an item, with none claimed twice.

| Part | Promises | Claimed by |
| --- | --- | --- |
| (1) base path | path joined to the wire path with the base path replacing `/v1`; no-path base unchanged; `Host` stays the authority; refuse query, fragment, userinfo and `..` | `story:hosted-base-path` ("The base-path rule", acceptance 1-4) |
| (2) static headers | map applied to the upstream head; refuse hop-by-hop and framing headers, `authorization` and the credential header; refuse CR/LF | `story:hosted-static-headers` ("The rule", acceptance 1-3) |
| (3) ingress auth | `bearer` or `none`, default `bearer`; `owner_secret_file` only for bearer; wire paths unauthenticated under `none`; owner-only paths decided and documented; startup warning; core dependency-free | `story:ingress-auth-optional` ("The rule", acceptance 1-5) |

Translation and `llm.routes/1` loading are not claimed by any of the three. `story:hosted-base-path` and `story:ingress-auth-optional` name them as not in scope.

What I could not establish:
- `epic:gateway-features` says nothing about Milestone 2, and its "Done when" lists none of the three stories. That is the epic's record of the approved plan, not a defect in the stories. I did not count it as a finding.
- Out of my lane, for the design and parallel-safety critics:
  - `story:ingress-auth-optional` says it "runs after" the two hosted stories but has no `depends_on` relation. `story:hosted-static-headers` does have one on `story:hosted-base-path`.
  - Its body lists `README.md` in "Scope", but the frontmatter scope omits it.

```findings
[]
```
