---
format: aep.planning-md/3
id: review-result:adversary-w06-public-model-listing-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w06 adversary, story:public-model-listing, pass 1
relations:
- reviews: story:public-model-listing
revision: 1
---
## Report

unit: story:public-model-listing, tree lg-w06-listing at d1a0e1a (implementation head 3f4de25)
verdict: INFEASIBLE (1 red case; no named client reaches it)
cases: executed 156→157, red 1
origin: introduced 1 / pre-existing 1 / undecided 0
wrote-outside-worktree: 1 (~/.cache/llm-gateway-w06/listing/adversary/suite.log)
needs-coordinator: none

Case added: `crates/llm-gateway/tests/adversary_public_listing.rs`,
`adversary_r1_a_user_agent_that_is_not_utf8_gets_plain_text`, committed as `d1a0e1a test: GET /
answers a non-UTF-8 user-agent with plain text (red)`. Run alone it fails with
`HTTP/1.1 400 Bad Request` and `{"error":{"code":"request-malformed","message":"the request is
not well-formed HTTP"}}`. Suite after the case: 156 passed, 1 failed (EXIT=101); clippy and fmt
clean.

Attacked without a break: owner-only facts in either answer (four user-agent kinds, GET and HEAD,
with and without the credential); any path from the public routes to the target source (shedding,
drain, missing relay); the unauthenticated surface (`//`, trailing slash, case, `#`,
percent-encoding, other methods get credential-absent; with the credential 405 with
`allow: GET, HEAD`; `/v1/routes` and `/metrics` still need the credential); echoed host (quote,
`$`, `<`, `/`, `@`, space rejected and the listener address used; head limit 8192 bytes);
user-agent precedence, case, trimming and empty value; acceptance 9–11 against design § 5; the
refusal order in the document, the specification and `decide()`.

| # | file:line | finding | verdict | origin | what reaches it |
|---|---|---|---|---|---|
| 1 | crates/llm-gateway/src/server.rs:428 | `read_head` turns a head that is not UTF-8 into `request-malformed`, so a non-UTF-8 `user-agent` on `GET /` gets 400, not the plain text of acceptance 2 | INFEASIBLE | introduced | no named client: Claude Code, Codex, browsers and curl send ASCII user-agents |
| 2 | docs/gateway.md:130 | the documented causes of `request-malformed` omit a head that is not UTF-8, which the code refused before this unit | CONFIRMED | pre-existing | any client sending obs-text in any header |

```findings
- file: crates/llm-gateway/src/server.rs
  line: 428
  category: acceptance
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a user-agent that is not UTF-8 makes GET / answer 400 request-malformed instead of the plain-text answer acceptance 2 gives to any other user-agent
- file: docs/gateway.md
  line: 130
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the documented causes of request-malformed omit a request head that is not UTF-8, which read_head refuses
```
