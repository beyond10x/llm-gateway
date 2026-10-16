---
format: aep.planning-md/3
id: review-result:critic-design-w11-round-2
kind: review-result
status: active
title: design critic, wave 2026-10-16-w02, round 2
relations:
- reviews: story:gateway-translation
- reviews: story:routes-document
revision: 1
---
approve

Both stories hold together as a set. I found no defect in the five categories.

- **Cycle:** none. I walked 11 edges outside the set as well as inside it. Both stories decompose `epic:gateway`, and both serve `vision:portable-model-inference`. `story:gateway-translation` is `informed_by` `story:modal-hosting`, which is not a needs-first edge. `story:routes-document` `depends_on` `story:gateway-translation`. The upstream stories it reuses (`story:hosted-base-path`, `story:hosted-static-headers`, `story:ingress-auth-optional`) are already `implemented`, and none of them points back at either story.
- **Chain:** the set has a single `depends_on` edge, from routes to translation. It has two written reasons: routes needs translation's cross-wire path and listing, and the two share `relaying.rs`, `hosted.rs`, `conformance/src/gateway.rs` and the spec files (routes-document Scope section). I would keep the edge rather than ask for it to be removed.
- **Split abstraction:** none. The seam is clean. Translation takes whatever target the source hands out (design 3, "a pod, a hosted endpoint"), and routes only adds a new source of hosted endpoints and aliases. Translation's acceptance 1–9 never read anything routes produces, and its fixtures are loopback upstreams, so it can be shown working alone. Routes consumes the translator and does not redefine it.
- **Hidden dependency:** none. I traced 7 acceptance reads to a producer.

  | Reader | Reads | Producer | Edge recorded |
  |---|---|---|---|
  | routes acceptance 1 (cross-wire translated) | the translator path | `story:gateway-translation` | yes |
  | routes acceptance 2 (all three wires listed) | translation's listing change | `story:gateway-translation` | yes |
  | routes Scope ("the translation page") | the page translation creates | `story:gateway-translation` | yes |
  | routes acceptance 5 (`GET /v1/routes`) | the existing route | `story:public-model-listing` and `spec/domains/gateway.yaml` | not needed, already landed |
  | routes acceptance 3 (hosted alias clash) | `[hosted.*]` | `story:hosted-endpoints` | not needed, already landed |
  | routes rule 4 (base-path rule) | `story:hosted-base-path` | `story:hosted-base-path` | not needed, `implemented` |
  | translation acceptance 7 (k11 scenario) | the existing scenario | existing, listed in its Scope | not needed |

  Of the 3 reads that point into this set, all 3 have an edge. The other 4 are produced outside it by stories that have already landed.
- **Horizontal slice:** none. Translation is a vertical slice, from decode through the upstream turn to the encoded answer, with its own matrix, refusals, listing and docs. Routes is a vertical slice from the deployment document to a served alias, with its own conformance scenarios and CLI tests.
- **Validator:** `aep plan artifact validate` reports `valid`. Its one note, on `review-result:adversary-w08-effort-default-pass-1`, is not part of this set and not my finding.
- **Out of my lane:**
  - Parallel safety: the routes Scope says it runs after translation in the same wave, and the overlapping files are named.
  - Acceptance: translation acceptance 8 asks for each refused-field statement to be backed by a decoder test, which is for the acceptance critic to judge.
  - Scope coverage: translation's Context says "No downstream tool execution", and nothing in either story contradicts it.

```findings
[]
```
