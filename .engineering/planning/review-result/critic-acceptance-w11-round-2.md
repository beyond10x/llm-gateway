---
format: aep.planning-md/3
id: review-result:critic-acceptance-w11-round-2
kind: review-result
status: active
title: acceptance critic, wave 2026-10-16-w02, round 2
relations:
- reviews: story:gateway-translation
- reviews: story:routes-document
revision: 1
---
approve

What I read: 2 of 2 ids (`story:gateway-translation` at revision 32, `story:routes-document` at revision 18). I ran `aep plan artifact show` on each, plus `show epic:gateway`, `lifecycle story`, `show review-result:critic-acceptance-w11-round-1` and `findings` on both stories. I also checked the tree with `git grep`.

Round 1 returned `approve` with `[]`, and I held to its reading of the numbered-list form. The revision added items 8 and 9 to the translation story and item 5 to the routes story.

- **Translation item 8 (published subset):** observable. It names two artifacts (`docs/gateway.md` and a site page) and four content headings. Each statement about a refused field must be backed by a test that feeds the request to the decoder.
- **Translation item 9:** each clause has an observable check. The non-streamed answer is a JSON object. The fixture upstream's received requests show no effort mapping and no `count_tokens` call. A request that breaks a subset rule gets `400 translation-invalid`.
- **Routes item 5:** `GET /v1/routes` exists in the tree (`crates/llm-gateway-cli/tests/binary.rs:288`, `crates/llm-gateway-cli/tests/adversary.rs:231`). The check "names each route's protocol and none of its URL, credential file or credential" is a string assertion on the response.
- **Transitions:** none missing. Before the work, a cross-wire request got `wire-not-served`. After it, the request is translated, and the `k11` scenario is kept only for a relay without a translator. Likewise, a `routes` key was refused before and now starts the gateway.
- **Scenario named by acceptance 7:** `contracts/gateway/scenarios/k11-a-wire-the-model-does-not-declare-is-refused-without-waking-a-target.yaml` exists.

What I could not establish:
- **Numbered-list form:** both acceptances are numbered lists, not single sentences (9 and 5 items). I did not flag this, as in round 1. If the one-sentence rule is meant strictly, it is a defect for both.
- **Subset rule vs outside-subset field:** item 9's "a request breaking a subset rule" and item 3's "a field outside llm's subset" are told apart only by the docs page of item 8. This is thin, but I could not name a fix beyond what the page already does, so I did not flag it.
- **Acceptance breadth:** no check covers non-streamed `translation-unrepresentable` (design 6). None covers capability bounding (routes rule 5) or the "path outside the base-path rule" refusal (routes rule 4). That is breadth, which is outside my lane.
- **Missing scenarios:** no `translate-*` or `routes-*` scenario files exist yet, which is expected while the stories are `proposed` and `draft`.
- **Other critics' lanes:** the `depends_on story:gateway-translation` edge, the shared surfaces (`relaying.rs`, `hosted.rs`, `checks/conformance/src/gateway.rs`) and the `informed_by story:modal-hosting` edge belong to the design and parallel-safety critics.

```findings
[]
```
