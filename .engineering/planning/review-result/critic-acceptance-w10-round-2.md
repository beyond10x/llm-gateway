---
format: aep.planning-md/3
id: review-result:critic-acceptance-w10-round-2
kind: review-result
status: active
title: Acceptance critic, wave 2026-10-16-w01 new stories, round 2
relations:
- reviews: story:hosted-base-path
- reviews: story:hosted-static-headers
- reviews: story:ingress-auth-optional
revision: 1
---
needs-revision

story:hosted-base-path — acceptance 2 says a `base_url` "with `/v1` reaches the same paths as before this story", but today `/v1` is refused as `config:value`, so no "before" exists to compare against. The criterion should drop `/v1` from the "unchanged" group and give it its own check: it was refused and now reaches `/v1/chat/completions` — `.engineering/planning/story/hosted-base-path.md:87` (`crates/llm-gateway-cli/src/hosted.rs:106-131`, where the `/` in `authority.contains(['/', …])` refuses any path)

Read 3 stories (`story:hosted-base-path`, `story:hosted-static-headers`, `story:ingress-auth-optional`) plus the parent epic, with `aep plan artifact show <id>`. I did not run `aep plan artifact kinds` or `aep plan artifact lifecycle`.

Not established:
- Whether round 1 raised the `/v1` finding. I judged the current text only.
- Round 2's change, `depends_on` plus a `## Depends on` section on `story:ingress-auth-optional`, touches no acceptance text. The other two stories' acceptances (`story:hosted-static-headers`, `story:ingress-auth-optional`) are observable and name a before and after, so I raise no finding on them.
- The ordering and shared-file claims are out of my lane (design and parallel-safety critics).

```findings
- file: .engineering/planning/story/hosted-base-path.md
  line: 87
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 2 says a base_url with /v1 'reaches the same paths as before this story', but today a base_url with any path is refused config:value (crates/llm-gateway-cli/src/hosted.rs:106-131), so there is no 'before' to compare; /v1 needs its own transition check (refused before, reaches /v1/chat/completions after)"
```
