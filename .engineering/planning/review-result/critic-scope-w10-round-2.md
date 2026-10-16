---
format: aep.planning-md/3
id: review-result:critic-scope-w10-round-2
kind: review-result
status: active
title: Scope critic, wave 2026-10-16-w01 new stories, round 2
relations:
- reviews: story:hosted-base-path
- reviews: story:hosted-static-headers
- reviews: story:ingress-auth-optional
revision: 1
---
approve

All three stories cover the operator's Milestone 2 part A promises, and none reaches past it.

Coverage, by promise (12 extracted, 12 traced):

| Promise | Claimed by |
|---|---|
| Base path allowed; upstream URL is base path plus wire path | hosted-base-path, "The base-path rule" and acceptance 1 |
| Base path replaces `/v1`; a base without a path keeps current behaviour | hosted-base-path, acceptance 2 |
| Host stays correct | hosted-base-path, acceptance 3 |
| Refuse query, fragment, userinfo and `..` | hosted-base-path, acceptance 4 |
| Static `headers` map applied to the upstream head | hosted-static-headers, acceptance 1 |
| Refuse hop-by-hop and framing headers, `authorization` and the credential header | hosted-static-headers, "The rule" and acceptance 2 |
| Refuse CR/LF in a header | hosted-static-headers, acceptance 2 |
| `ingress_auth` is `bearer` or `none`, default bearer | ingress-auth-optional, "The rule" and acceptance 3 |
| `owner_secret_file` required only for bearer | ingress-auth-optional, acceptance 3 |
| With `none`, wire paths need no authorization | ingress-auth-optional, acceptance 1 |
| Owner-only paths: decide and document | ingress-auth-optional, "The rule" (still refused without a secret) and acceptance 2 |
| Startup warning; core stays dependency-free | ingress-auth-optional, acceptance 4 and 5 |

- **Reach beyond the request:** none. The extra limits are refusals in the same spirit as the request: segment limits and `%`/`\` in the base path, and the 32-entry and 1024-byte caps on headers. The docs, README and website files in scope all document the three features.
- **Duplicate claims:** none. Each outcome belongs to exactly one story.
- **Narrowed promises:** none. Translation and `llm.routes/1` loading are excluded explicitly (hosted-base-path "Not in scope").
- **Round 1:** the `depends_on` edge, the `## Depends on` section and the README in ingress-auth's scope are all present.

What I read: the three stories in full via `aep plan artifact show`, `aep plan artifact graph` filtered to them, and the critic rubric. I did not read the operator's request as a stored artifact, so I took its promises from your message.

What I could not establish: none. Whether the acceptances can be checked belongs to the acceptance critic, and I did not judge it.

```findings
[]
```
