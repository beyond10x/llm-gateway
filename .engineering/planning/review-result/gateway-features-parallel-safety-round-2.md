---
format: aep.planning-md/3
id: review-result:gateway-features-parallel-safety-round-2
kind: review-result
status: active
title: Parallel-safety critic, epic:gateway-features, round 2
relations:
- reviews: story:gateway-observability
- reviews: story:gateway-deployment
- reviews: story:cold-start-hold
- reviews: story:runpod-production-transport
- reviews: story:gateway-translation
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: story:ess-0-53
revision: 1
---
needs-revision

story:gateway-observability — it adds a per-call record and conformance observation, and counts on the relay path (`crates/llm-gateway/src/relay.rs`), the same relay and conformance files that story:target-fallback and story:hosted-endpoints list in their Scope (`relay.rs`, `checks/conformance`, `contracts/gateway/scenarios`); "Neighbours" names only story:gateway-deployment and story:cold-start-hold, and no edge joins it to either story (cited, both bodies, `checks/conformance/src/gateway.rs` holds `LastRelay`). Remedy: an ordering edge naming the shared file, or splitting the surface so they no longer share it. — .engineering/planning/story/gateway-observability.md:40

story:gateway-deployment — it and story:cold-start-hold both land on `crates/llm-runpod/src/config.rs` (rows B9 `:72-75` and K28 `:76-77`) and on the relay hooks in `crates/llm-gateway/src/relay.rs` (B8 `:581-595` and L6). Neither body admits it, there is no edge between them, and the observability "Neighbours" note covers the pair only as "any of the three" with no surface named (cited, via matrix rows; story:hosted-endpoints says the same about `config.rs` and `serve.rs` at `.engineering/planning/story/hosted-endpoints.md:58`). Remedy: an ordering edge recording the shared file, or splitting the surface. — docs/llmgw-capability-matrix.md:132-133

story:runpod-production-transport — its body names no surface; it is placeable only through `crates/llm-runpod/src/transport.rs`, which `spec/domains/runpod.yaml:11` cites (cited, one step removed). That puts it in `crates/llm-runpod`, where story:gateway-deployment (K7 `lib.rs:26-28`, B9 `request.rs`, `config.rs`) and story:cold-start-hold (K28 `config.rs`, L6, L13, L15 `pool.rs`) also land. No scope, no edge and no mention in either body; the file-level overlap with `lib.rs` is inferred. Remedy: a typed scope plus an ordering edge naming the shared file, or leave it out of any concurrent set. — .engineering/planning/story/runpod-production-transport.md:12

story:gateway-translation — it claims the whole of `crates/llm-gateway` and `contracts/gateway` (inferred), which every other story in the set also changes. Its own line says to "coordinate changes through their owning story" but names no story and has no edge; "test fallback visibility boundary" and "usage" in its Context also overlap story:target-fallback and story:usage-records. A scope this wide forbids parallel work with everything. Remedy: narrow the scope to the files it changes with an ordering edge to each overlapping story, or leave it out of any concurrent set. — .engineering/planning/story/gateway-translation.md:30

Pairs I judged safe or already admitted:
- story:hosted-endpoints to story:gateway-deployment and story:cold-start-hold, story:target-fallback to story:hosted-endpoints, and story:usage-records to story:target-fallback and story:gateway-observability all have `depends_on` edges with the shared files named. Their other pairs (for example story:usage-records with story:hosted-endpoints) are ordered through those edges.
- story:gateway-observability to story:gateway-deployment and story:cold-start-hold is admitted in "Neighbours", in prose rather than as edges.
- story:ess-0-53 declares in "Ordering" that it runs alone.

What I read: 9 artifacts via `aep plan artifact show` plus `aep plan artifact graph`, the capability-matrix rows the bodies cite, `relay.rs`, `spec/domains/*.yaml`, and `checks/conformance/src/gateway.rs`. I did not read any `review-result`.

Surface counts: cited 7 (story:hosted-endpoints, story:target-fallback, story:usage-records, story:gateway-observability, story:ess-0-53, story:gateway-deployment and story:cold-start-hold via matrix rows). Inferred 1 (story:gateway-translation). Placed only one step removed 1 (story:runpod-production-transport). Unplaceable 0.

What I could not establish:
- The `PLANNED` and `UNMAPPED` markers that story:hosted-endpoints and story:target-fallback cite in `spec/domains/` did not appear in my `git grep`. The worktree has uncommitted changes, so I judged the artifacts as written.
- Whether the shared file `docs/llmgw-capability-matrix.md` conflicts when story:gateway-observability, story:gateway-deployment and story:cold-start-hold edit different rows is a merge-mechanics question I did not test.
- Whether story:gateway-deployment's inference call (B8) requires story:runpod-production-transport is a design question, out of my lane.

```findings
- file: .engineering/planning/story/gateway-observability.md
  line: 40
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "story:gateway-observability adds the per-call record, the conformance observation and relay-path counting on crates/llm-gateway/src/relay.rs and checks/conformance, which story:target-fallback and story:hosted-endpoints also list in their Scope; Neighbours names only story:gateway-deployment and story:cold-start-hold and no edge joins them (the story:hosted-endpoints overlap on conformance files is inferred from its scope list)"
- file: docs/llmgw-capability-matrix.md
  line: 132
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: pre-existing
  message: "story:gateway-deployment (rows B8, B9, K7) and story:cold-start-hold (rows L6, K28, L13, L15) both land on crates/llm-runpod/src/config.rs and the relay hooks in relay.rs, neither body admits it and no edge joins them; the observability note covers the pair only as 'any of the three' with no surface"
- file: .engineering/planning/story/runpod-production-transport.md
  line: 12
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: "story:runpod-production-transport names no surface in its body; via spec/domains/runpod.yaml:11 it lands on crates/llm-runpod/src/transport.rs, in the crate where story:gateway-deployment and story:cold-start-hold also land, with no scope and no edge (file-level overlap on lib.rs is inferred)"
- file: .engineering/planning/story/gateway-translation.md
  line: 30
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: "story:gateway-translation claims all of crates/llm-gateway and contracts/gateway (inferred), which every other story in the set changes; it points at 'owning stories' without naming one or recording an edge, so it forbids parallel work with the whole set"
```
