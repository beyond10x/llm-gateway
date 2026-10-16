---
format: aep.planning-md/3
id: review-result:critic-parallel-w10-round-1
kind: review-result
status: active
title: Parallel-safety critic, wave 2026-10-16-w01 new stories, round 1
relations:
- reviews: story:hosted-base-path
- reviews: story:hosted-static-headers
- reviews: story:ingress-auth-optional
revision: 1
---
needs-revision

- story:ingress-auth-optional — names its collisions with both hosted stories on `config.rs`, `deployment.yaml`, `gateway.yaml`, the conformance adapter and the docs, and says "it runs after them", but records no `depends_on` edge to `story:hosted-static-headers` or `story:hosted-base-path` (cited, all three bodies). The store therefore waves ingress-auth in wave 2 and static-headers in wave 4, the reverse of the coordinator's serial order. Either add the ordering edge, naming the shared files as its reason, or split the shared surface so the two no longer share it — `aep plan artifact show story:ingress-auth-optional` (`relations` lists no `depends_on`); `aep plan artifact waves --kind story --status draft` (wave 2 vs wave 4)

What was fine:
- `story:hosted-static-headers` has its `depends_on story:hosted-base-path` edge, and its "Depends on" section names the shared surfaces.
- `story:hosted-base-path` states in its body that the three run one after another, and the edge to static-headers covers that pair.

Read: 4 artifacts (the three stories and `epic:gateway-features`), via `aep plan artifact show` and `aep plan artifact waves --kind story --status draft`.

Surfaces: 3 items cited, 0 inferred, 0 unplaceable. Acceptance reads traced to a producer in the set: 0, so none needs an edge. The static-headers acceptance reads only its own start-time config, and ingress-auth reads nothing the hosted stories write.

Not established:
- Collisions with `story:target-fallback` and `story:usage-records` that `waves` reports are outside the set, so I did not judge them.
- I did not run `aep plan artifact graph` or `relations`, so I did not look for a `depends_on` edge anywhere other than in the `show` output.
- `README.md` appears only in ingress-auth's scope. That is a scope question, not a concurrency one, and out of my lane.

```findings
- file: "aep plan artifact show story:ingress-auth-optional"
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "story:ingress-auth-optional names its shared files with story:hosted-base-path and story:hosted-static-headers and says it runs after them (cited), but records no depends_on edge to either, so waves puts it in wave 2 and static-headers in wave 4, contrary to the intended serial order; add the ordering edge naming the shared files as its reason, or split the shared surface"
```
