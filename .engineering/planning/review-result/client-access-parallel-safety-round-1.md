---
format: aep.planning-md/3
id: review-result:client-access-parallel-safety-round-1
kind: review-result
status: active
title: Parallel-safety critic, epic:client-access, round 1
relations:
- reviews: epic:client-access
- reviews: story:model-tool-calling
- reviews: story:pod-proxy-tls
- reviews: story:client-qualification
- reviews: story:claude-code-wire
- reviews: story:loom-qualification
- reviews: story:public-model-listing
revision: 1
---
needs-revision

1. story:public-model-listing — its "Client profiles" section offers a profile only for a model whose tool calling is `Parsed`, the declaration story:model-tool-calling creates, and no `depends_on` edge runs from it to that story. The two also both edit `docs/gateway.md`, `spec/domains/clients.yaml`, `crates/llm-gateway-cli/src/serve.rs` and `crates/llm-gateway/tests/gateway.rs`. The missing edge is the fix, or move the section into its own story. — .engineering/planning/story/public-model-listing.md:97, .engineering/planning/story/model-tool-calling.md:40-44
2. story:claude-code-wire — it and story:public-model-listing both edit `docs/gateway.md` and `spec/domains/clients.yaml` (cited), and `crates/llm-gateway/src/server.rs` (cited by the listing, inferred for this one). Criterion 2 may add `/api/hello` to the unauthenticated surface, and the listing's criterion 7 rewrites the same sentence, `docs/gateway.md:100`. Neither body names the other and no edge orders them. Fix: an ordering edge recording the file, or splitting that surface. — .engineering/planning/story/claude-code-wire.md:58-59, .engineering/planning/story/public-model-listing.md:70-71, docs/gateway.md:100
3. story:model-tool-calling — it and story:pod-proxy-tls both land on `spec/domains/deployment.yaml` (cited by both bodies) and on `crates/llm-gateway-cli/tests/config.rs` and `crates/llm-gateway-cli/src/serve.rs` (cited by one, inferred by the other). Their dependency chains (`provider-key-files` and `gateway-deployment`) never meet, and neither body names the other. Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/model-tool-calling.md:73-74, .engineering/planning/story/pod-proxy-tls.md:47
4. story:model-tool-calling — it and the existing drafts story:hosted-endpoints and story:target-fallback all land on `spec/domains/deployment.yaml` (cited by all three), with `crates/llm-gateway-cli/src/config.rs` and `crates/llm-gateway-cli/src/serve.rs` inferred. Neither side names the other, and no edge or transitive path orders them. Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/model-tool-calling.md:73, `aep plan artifact waves --kind story --status draft` collision lines for story:hosted-endpoints and story:target-fallback
5. story:model-tool-calling — it and the existing draft story:gateway-translation both change `docs/gateway.md` and `spec/domains/gateway.yaml` (cited by both) and the `RefusalCode` set in `crates/llm-gateway/src/error.rs` (inferred, on this story's side: the refusal is added there). Neither body names the other and no path orders them. Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/model-tool-calling.md:40-44 and :70-71, .engineering/planning/story/gateway-translation.md:80
6. story:claude-code-wire — its header-forwarding change cites `crates/llm-gateway/src/relay.rs:582-587`, and story:target-fallback cites the same file (`:86-94`). The existing drafts story:gateway-translation, story:hosted-endpoints, story:usage-records and story:gateway-observability also edit `relay.rs` (cited or inferred), and `docs/gateway.md` is cited by gateway-translation. The collisions are unnamed and no edge or path orders them. Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/claude-code-wire.md:41-43, .engineering/planning/story/gateway-translation.md:80
7. story:client-qualification — acceptance 1 and 2 read "the request lines the gateway received" and the cold-start time "from the gateway's standard error with UTC times". Today the binary writes only listening, refused and stopped lines (`spec/domains/deployment.yaml:14-18`). The matrix row O3 says "No crate in llm-gateway logs", and story:gateway-observability is the only story that adds logging. No `depends_on` edge runs to it, so one wave could hold both. The producer is inferred: that story's body does not name request lines or cold-start times. story:loom-qualification line 32 reads the same request lines. Fix: add the missing edge, or have those criteria read another source. — .engineering/planning/story/client-qualification.md:36-39, .engineering/planning/story/loom-qualification.md:32, docs/llmgw-capability-matrix.md:163
8. story:pod-proxy-tls — it and the existing draft story:live-runpod-wiring both edit `crates/llm-gateway-cli/Cargo.toml`, `crates/llm-gateway-cli/src/serve.rs`, `crates/llm-gateway-cli/tests/binary.rs` and `spec/domains/deployment.yaml`. The surfaces are inferred on the live-runpod-wiring side (all five of its scope entries are inferred). Each composes the binary's Runpod pieces, neither names the other, and no edge orders them (both feed story:client-qualification). Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/pod-proxy-tls.md:62-63, `aep plan artifact show story:live-runpod-wiring` scope
9. story:model-tool-calling — it and the existing drafts story:gateway-deployment and story:cold-start-hold both edit `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway-cli/src/serve.rs` and `crates/llm-gateway/tests/wire_relay.rs`. The refusal is decided "before any target is asked for", on the path those stories change. Every surface here is inferred, on both sides. Fix: an ordering edge recording the file, or splitting the surface. — .engineering/planning/story/model-tool-calling.md:42-44, `aep plan artifact waves --kind story --status draft` collision lines for story:gateway-deployment and story:cold-start-hold

**What you read:** 6 set items plus epic:client-access, with story:gateway-deployment, story:cold-start-hold, story:live-runpod-wiring, story:gateway-observability, story:gateway-translation, story:hosted-endpoints and story:target-fallback for the comparison. Commands: `aep plan artifact show` on each, `aep plan artifact waves --kind story --status draft`, `aep plan artifact graph`, `aep plan artifact relations`, and a transitive `depends_on` check of every reported collision pair. I also read `docs/gateway.md:80-110`, `spec/domains/clients.yaml` and `spec/domains/deployment.yaml:10-22`. Surfaces: 6 items cited (each also carries inferred entries), 0 inferred-only, 0 unplaced. Acceptance reads traced to a producer in the set: 11, of which `depends_on` records 8 (missing: client-qualification to gateway-observability, loom-qualification to gateway-observability, public-model-listing to model-tool-calling).

**What I could not establish:**
- Whether story:gateway-observability's O3 logging will emit request lines or cold-start times. The body is silent, so finding 7 is inferred on the producer side.
- The `scope` entries of the existing drafts (cold-start-hold, gateway-deployment, live-runpod-wiring and others) are mostly inferred, so findings 6, 8 and 9 rest on inferred surfaces.
- `aep plan artifact waves` places each reported pair in different waves, so a wave run keeps them apart. That order comes from scope and is not recorded in a body or an edge, and it moves if a scope changes.
- Out of my lane, and not setting the verdict:
  - story:pod-proxy-tls and story:runpod-production-transport each add a TLS client for the binary side and the transport side. Whether that is one design is the design critic's call.
  - story:model-tool-calling criterion 5 invents a default whose value the acceptance does not name.

```findings
- file: .engineering/planning/story/public-model-listing.md
  line: 97
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its Client profiles section is offered only for a model whose tool calling is Parsed, the declaration story:model-tool-calling creates, and no depends_on edge runs to it; the two also share docs/gateway.md, spec/domains/clients.yaml, crates/llm-gateway-cli/src/serve.rs and crates/llm-gateway/tests/gateway.rs"
- file: .engineering/planning/story/claude-code-wire.md
  line: 58
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and story:public-model-listing both edit docs/gateway.md and spec/domains/clients.yaml (cited) and crates/llm-gateway/src/server.rs (inferred on this side), including the docs/gateway.md:100 unauthenticated-paths sentence, and neither body names the other; add an ordering edge recording the file or split the surface"
- file: .engineering/planning/story/model-tool-calling.md
  line: 73
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and story:pod-proxy-tls both land on spec/domains/deployment.yaml (cited by both) and on crates/llm-gateway-cli/tests/config.rs and crates/llm-gateway-cli/src/serve.rs, their depends_on chains never meet, and neither body names the other; add an ordering edge recording the file or split the surface"
- file: .engineering/planning/story/model-tool-calling.md
  line: 73
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and the existing drafts story:hosted-endpoints and story:target-fallback all land on spec/domains/deployment.yaml (cited by all three) with config.rs and serve.rs inferred, and no edge or path orders them; add an ordering edge recording the file or split the surface"
- file: .engineering/planning/story/model-tool-calling.md
  line: 40
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and the existing draft story:gateway-translation both change docs/gateway.md and spec/domains/gateway.yaml (cited by both) and the RefusalCode set in error.rs (inferred), and no edge or path orders them; add an ordering edge recording the file or split the surface"
- file: .engineering/planning/story/claude-code-wire.md
  line: 41
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its relay.rs:582-587 header change shares crates/llm-gateway/src/relay.rs (cited) and docs/gateway.md with the existing drafts story:target-fallback, story:gateway-translation, story:hosted-endpoints, story:usage-records and story:gateway-observability, and no edge or path orders them; add an ordering edge recording the file or split the surface"
- file: .engineering/planning/story/client-qualification.md
  line: 36
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "acceptance 1 and 2 read gateway stderr request lines and UTC cold-start times that only story:gateway-observability (matrix row O3) would produce, and no depends_on edge runs to it; the producer is inferred because that story's body does not name request lines or cold-start times; add the edge or read another source"
- file: .engineering/planning/story/pod-proxy-tls.md
  line: 62
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it and the existing draft story:live-runpod-wiring both edit crates/llm-gateway-cli/Cargo.toml, src/serve.rs, tests/binary.rs and spec/domains/deployment.yaml with no ordering edge; inferred, because every live-runpod-wiring scope entry is inferred"
- file: .engineering/planning/story/model-tool-calling.md
  line: 42
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it and the existing drafts story:gateway-deployment and story:cold-start-hold both edit crates/llm-gateway/src/relay.rs, crates/llm-gateway-cli/src/serve.rs and crates/llm-gateway/tests/wire_relay.rs with no ordering edge; inferred, because every surface is inferred on both sides"
```
