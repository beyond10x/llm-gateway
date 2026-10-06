---
format: aep.planning-md/3
id: review-result:gateway-features-parallel-safety-round-1
kind: review-result
status: active
title: Parallel-safety critic, epic:gateway-features, round 1
relations:
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
- reviews: story:ess-0-53
- reviews: story:gateway-translation
- reviews: story:runpod-production-transport
- reviews: story:gateway-observability
revision: 1
---
needs-revision

story:hosted-endpoints — this story and story:gateway-deployment (rows K7, D3, B8) both change `crates/llm-gateway-cli/src/config.rs` (`ProviderKind`, `Provider`) and `serve.rs` (the target source, which is not wired today); story:cold-start-hold (row K28) edits the same document file; none of the three says so and no edge joins them (cited for this story; inferred for the other two via the matrix rows); either record the shared files as the reason for an ordering edge, or split the document and target-source changes so they no longer share a file — .engineering/planning/story/hosted-endpoints.md:51
story:target-fallback — this story and story:usage-records both claim `crates/llm-gateway/src/relay.rs`, and both changes land in the one function `relay::serve` (the acquire, send, read-head and stream_answer sequence); neither body mentions the other and no edge joins them (cited, both Scope sections); either add an ordering edge naming that function, or split the answer-reading path from the attempt loop — .engineering/planning/story/target-fallback.md:62
story:target-fallback — the story changes the single-target `RelayTargets::acquire(alias)` port (`crates/llm-gateway/src/relay.rs:86-90`, called at `:573`) that story:cold-start-hold (rows L6, W6) must also change to hold a request, and that story:gateway-deployment (row B8) must implement over the Runpod pool; neither of the others is named or linked (inferred for them, cited for this story's "target-source port"); record the port as the shared reason on an edge or split the port change out — .engineering/planning/story/target-fallback.md:62
story:ess-0-53 — it regenerates all of `contracts/` (`suite.json`, `schema/`) and rewrites `Cargo.lock` and `checks/conformance/Cargo.toml` in one commit, which collides with every story that adds scenarios or a dependency (story:hosted-endpoints, story:target-fallback, story:usage-records, each listing `contracts/` and `checks/conformance`); its "scenario totals are unchanged" test fails if any of them lands first, and the body names none of them (cited for this story, cited for the three new stories' Scope sections); add an ordering edge recording the generated files as the reason, or leave this story out of any concurrent set — .engineering/planning/story/ess-0-53.md:31
story:gateway-translation — its scope is two whole trees (`crates/llm-gateway`, `contracts/gateway`), so it collides with every item that touches `relay.rs` (story:target-fallback, story:usage-records) and its acceptance includes "usage", which story:usage-records also claims; the body's own warning says not to infer parallel safety, but it names no narrower surface and no neighbour (inferred, whole crate); narrow the surface to files, or leave it out of any concurrent set — .engineering/planning/story/gateway-translation.md:28
story:runpod-production-transport — the body cites no code surface and writes no acceptance, so it is unassessed rather than safe; the only trace is the DEFERRED note in `spec/domains/runpod.yaml:11` pointing at `crates/llm-runpod/src/transport.rs`, and the epic gives it "the B rows" that story:gateway-deployment also lists (B8, B9, K7), so both would edit the key-reading and target-source path unnamed (inferred); establish the surface and name the K7/B8/B9 split, or keep it out of the concurrent set — .engineering/planning/story/runpod-production-transport.md:12
story:gateway-observability — it adds `GET /metrics` (R4) and counters on the request path (O1, O2), while story:gateway-deployment adds routes `GET /` and `/v1/models` (R1, R5) in the same route table (`crates/llm-gateway/src/server.rs:384-443`, cited by rows R2, R3, R5) and story:cold-start-hold hooks the same relay path (`route_cold_start_holds_total` against W6/L6); no edges join the three and no body names the others (inferred for observability); record the shared files as the reason on edges or split them — .engineering/planning/story/gateway-observability.md:14

What I read: 10 artifacts (the three new stories plus the six named drafts, and epic:gateway-features), by `aep plan artifact show` and `aep plan artifact graph`. I also read `docs/llmgw-capability-matrix.md`, `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway-cli/src/{config,serve}.rs`, and the `PLANNED`/`UNMAPPED`/`DEFERRED` markers in `spec/domains/*.yaml`.

Surface counts for the 9 stories in the set:
- Cited: 4 (story:ess-0-53, story:target-fallback, story:runpod-production-transport via `runpod.yaml:11`, story:gateway-translation at crate level only).
- Inferred: 5 (story:hosted-endpoints, story:usage-records, story:gateway-observability, story:gateway-deployment, story:cold-start-hold). Their paths come from their own "Scope (inferred)" sections or from the matrix rows they cite, and I checked that the paths exist.
- Unplaceable: 0, though story:runpod-production-transport is barely placed.

What I could not establish:
- **Shared files in the set:** every story lists `contracts/gateway/scenarios`, `checks/conformance`, `docs/gateway.md` and `spec/domains/*.yaml`. I did not open these to check whether the edits merge cleanly (conformance is a single 1202-line `gateway.rs`). I did not make this a separate finding because the shared-file claim is inferred.
- **Pairs I found no collision for:** story:usage-records with story:hosted-endpoints, both only through those shared files; story:hosted-endpoints with story:target-fallback (an explicit `depends_on` edge exists); story:usage-records with story:gateway-observability (explicit edge).
- **Blocked story:** story:usage-records is blocked by `decision-blocker:usage-from-responses`, so it cannot start now whatever the collisions are.
- **Out of my lane, not counted toward the verdict:**
  - story:usage-records and story:gateway-translation both claim "usage" as an outcome. That is a split question for the design critic.
  - story:runpod-production-transport has no acceptance. That is for the acceptance critic.

```findings
- file: .engineering/planning/story/hosted-endpoints.md
  line: 51
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story and story:gateway-deployment (rows K7, D3, B8) both change crates/llm-gateway-cli/src/config.rs (ProviderKind, Provider) and serve.rs (the target source), and story:cold-start-hold (row K28) edits the same document file; none says so and no edge joins them (cited for this story, inferred for the other two via the matrix rows); either record the shared files as the reason for an ordering edge or split the document and target-source changes so they no longer share a file"
- file: .engineering/planning/story/target-fallback.md
  line: 62
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story and story:usage-records both claim crates/llm-gateway/src/relay.rs and both changes land in the function relay::serve; neither body mentions the other and no edge joins them (cited, both Scope sections); either add an ordering edge naming that function or split the answer-reading path from the attempt loop"
- file: .engineering/planning/story/target-fallback.md
  line: 62
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the story changes the single-target RelayTargets::acquire(alias) port (crates/llm-gateway/src/relay.rs:86-90, called at :573) that story:cold-start-hold (rows L6, W6) must also change and story:gateway-deployment (row B8) must implement; neither is named or linked (inferred for them, cited for this story's target-source port); record the port as the shared reason on an edge or split the port change out"
- file: .engineering/planning/story/ess-0-53.md
  line: 31
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "regenerating all of contracts/ and rewriting Cargo.lock and checks/conformance/Cargo.toml in one commit collides with every story that adds scenarios or a dependency (story:hosted-endpoints, story:target-fallback, story:usage-records), and the 'scenario totals are unchanged' check fails if any lands first; the body names none of them; add an ordering edge recording the generated files as the reason or leave the story out of any concurrent set"
- file: .engineering/planning/story/gateway-translation.md
  line: 28
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: "the scope is two whole trees (crates/llm-gateway, contracts/gateway), so it collides with every item touching relay.rs (story:target-fallback, story:usage-records) and its acceptance includes usage, which story:usage-records also claims; the body names no narrower surface and no neighbour (inferred, whole crate); narrow the surface to files or leave the story out of any concurrent set"
- file: .engineering/planning/story/runpod-production-transport.md
  line: 12
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: "the body cites no code surface and writes no acceptance, so it is unassessed rather than safe; the only trace is spec/domains/runpod.yaml:11 pointing at crates/llm-runpod/src/transport.rs, and the epic gives it the B rows that story:gateway-deployment also lists (B8, B9, K7), so both would edit the key-reading and target-source path unnamed (inferred); establish the surface and name the K7/B8/B9 split or keep it out of the concurrent set"
- file: .engineering/planning/story/gateway-observability.md
  line: 14
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: "it adds GET /metrics (R4) and request-path counters (O1, O2) while story:gateway-deployment adds GET / and /v1/models (R1, R5) in the same route table (crates/llm-gateway/src/server.rs:384-443) and story:cold-start-hold hooks the same relay path; no edges join the three and no body names the others (inferred for observability); record the shared files as the reason on edges or split them"
```
