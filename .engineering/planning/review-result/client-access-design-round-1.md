---
format: aep.planning-md/3
id: review-result:client-access-design-round-1
kind: review-result
status: active
title: Design critic, epic:client-access, round 1
relations:
- reviews: epic:client-access
- reviews: story:model-tool-calling
- reviews: story:pod-proxy-tls
- reviews: story:client-qualification
- reviews: story:claude-code-wire
- reviews: story:loom-qualification
- reviews: story:public-model-listing
- reviews: story:hosted-endpoints
revision: 1
---
needs-revision

- story:public-model-listing — its added "Client profiles" section offers a profile only for a model "whose tool calling is `Parsed`", which is the per-model declaration `story:model-tool-calling` creates, and no `depends_on` edge runs from it to `story:model-tool-calling` (the new `informed_by epic:client-access` edge records no ordering); add `depends_on story:model-tool-calling`, which closes no cycle — `.engineering/planning/story/public-model-listing.md:96-97`, `.engineering/planning/story/model-tool-calling.md:42-44`
- story:client-qualification — acceptance 1 and 2 read "the request lines the gateway received" and cold-start times "from the gateway's standard error with UTC times". Today the binary writes only listening, refused and stopped lines. The only story in the set's graph that adds request logging is `story:gateway-observability` (row O3). `story:client-qualification` has no `depends_on` edge to it, directly or through its four dependencies, so add `depends_on story:gateway-observability`; if that story does not write per-request, timestamped lines, then no story produces them and the set is missing one — `.engineering/planning/story/client-qualification.md:36-39`, `README.md:112-116`, `aep plan artifact graph`
- story:hosted-endpoints — the new `depends_on story:pod-proxy-tls` edge (`:13`) is not reflected in the body, so two stories still each claim the binary's TLS client. The "Depends on" section omits `story:pod-proxy-tls`, and the Scope lines still say "the TLS crate goes here" in `crates/llm-gateway-cli/Cargo.toml` (`:127`). They leave "which TLS crate" and "how case 4's fixture gets a trusted root (… a CA file in the document or a test-only root)" open (`:144`). `story:pod-proxy-tls` acceptance 1 and 3 already settle the test root as a seam and refuse any document trust-root key. State that `story:hosted-endpoints` reuses that client and seam, and drop the competing TLS and CA-file scope — `.engineering/planning/story/hosted-endpoints.md:96-106,127,144`, `.engineering/planning/story/pod-proxy-tls.md:36-48`

What I read: 14 artifacts, via `aep plan artifact show` for the 10 set items plus `story:cold-start-hold`, `story:gateway-observability`, `story:live-runpod-wiring` and `story:gateway-deployment`, and `aep plan artifact relations`, `graph` and `validate` (valid), plus the design document and `spec/domains/clients.yaml`. I walked all 229 edges, outside the set as well, with a script over the `depends_on` and `blocks` edges, and found no cycle. I traced 7 acceptance reads to a producer in the set, and an edge records 4 of them. The 3 with no edge are the two findings above plus the `hosted-endpoints` TLS overlap.

What I could not establish:
- Whether `story:gateway-observability` emits per-request lines with UTC timestamps; its body speaks of `RUST_LOG` logging and counters only.
- Out of my lane, and none of it set the verdict. `story:claude-code-wire`, `story:model-tool-calling` and `story:hosted-endpoints` share `relay.rs`, `checks/conformance/src/gateway.rs` and `docs/gateway.md` (parallel-safety). `story:claude-code-wire` criteria 2 to 4 are either/or (acceptance). The "Client profiles" section of `story:public-model-listing` has no numbered criteria (acceptance).
- Unease, not a finding: `story:client-qualification` holds Claude Code and Codex in one record, so a Codex failure delays `story:claude-code-wire`. It shares one pod start and one cost ceiling, so I left it.

```findings
- file: .engineering/planning/story/public-model-listing.md
  line: 96
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its added Client profiles section offers a profile only for a model whose tool calling is Parsed, which story:model-tool-calling creates, and no depends_on edge runs from it to story:model-tool-calling (the new informed_by epic:client-access edge records no ordering); add depends_on story:model-tool-calling"
- file: .engineering/planning/story/client-qualification.md
  line: 36
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance reads request lines and UTC cold-start times from the gateway's standard error, which only story:gateway-observability (row O3 logging) could produce, and no depends_on edge runs from it to story:gateway-observability; add the edge, or add the story that writes those lines"
- file: .engineering/planning/story/hosted-endpoints.md
  line: 96
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the new depends_on story:pod-proxy-tls edge is not reflected in the body: Depends on omits it and the Scope still puts the TLS crate, the trust-root decision and a possible CA-file key here, so two stories both claim the binary's TLS client; state that this story reuses the pod-proxy-tls client and test-root seam and drop the competing scope"
```
