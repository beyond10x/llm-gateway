---
format: aep.planning-md/3
id: review-result:security-sa-token-pass-1
kind: review-result
status: active
title: Wave 2026-11-21-w02 security review, story:gateway-sa-token-ingress, pass 1
relations:
- reviews: story:gateway-sa-token-ingress
revision: 1
---
unit: story:gateway-sa-token-ingress, independent security review pass 1, covering pull request 54 at 2ec6aa5 (wave/2026-11-21-w02)
verdict: needs fixes before merge (one blocker)
cases: crates/llm-gateway-cli/tests/token_review_conformance.rs, 6 written, 3 red at 26d91c7
origin: introduced 4, pre-existing 0, undecided 0
wrote-outside-worktree: none (the test file was left uncommitted in the tree)

Findings:
1. One review could last about twice review-timeout-ms: the deadline was checked only between reads, and every read waited the full limit (1.708 s under a 1 s bound). Blocker. Fixed in 961afdc: connect, handshake, write and every read get only what is left of one deadline.
2. status.error was never read; an answer with authenticated true and error set admitted. Fixed in 961afdc: a non-empty status.error is a rejection.
3. A caller with no valid token who knows the audience can send distinct JWT-shaped tokens; each takes one of the 8 shared review slots, and a listed caller without a cached admission is credential-review-unavailable meanwhile. The review judged it acceptable behind the guide's NetworkPolicy, needing no per-source limit before merge. A refuse-only pre-filter (not a JWT, exp passed, aud without the audience) costs such tokens no slot unless they name the audience. Accepted by the coordinator for this release and pinned by conformance_a_flood_naming_the_audience_holds_eight_reviews_and_refuses_only_uncached_callers_with_503; the follow-up is story:review-slot-fairness (draft).
4. Docs: the minimal tokenreviews role before system:auth-delegator, token replay on the plain-HTTP hop, ca_file read once at start, and the eviction rule. Fixed in 961afdc (a rejection no longer evicts an admission).

Held: no odd answer admits (17 answers), a rotated own token is presented from the next review on, the API host name is verified.

Decisions (coordinator): finding 3 is accepted behind a NetworkPolicy for this release; its test expectation was changed from "the listed caller is admitted" to the documented bound, and the finding stays recorded here.

```findings
[
{"file":"crates/llm-gateway-cli/src/token_review.rs","line":236,"category":"boundary","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"one review can last about twice review-timeout-ms: each read waits the full limit and the deadline is checked only between reads (tls.rs:309-315 sets the io timeout once)"},
{"file":"crates/llm-gateway-cli/src/token_review.rs","line":449,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"answered never reads status.error; an answer with authenticated true and error set admits"},
{"file":"crates/llm-gateway-cli/src/token_review.rs","line":213,"category":"concurrency","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"distinct JWT-shaped tokens from a caller with no valid token fill the 8 shared review slots and a listed caller without a cached admission gets 503; accepted behind the NetworkPolicy for this release, follow-up story:review-slot-fairness"},
{"file":"website/docs/guides/admit-service-account-tokens.md","line":73,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the guide shows system:auth-delegator before the minimal tokenreviews role, omits token replay on the plain-HTTP hop and that ca_file is read once at start; a rejection can evict an admission"}
]
```
