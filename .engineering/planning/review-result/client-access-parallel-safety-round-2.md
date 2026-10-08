---
format: aep.planning-md/3
id: review-result:client-access-parallel-safety-round-2
kind: review-result
status: active
title: Parallel-safety critic, epic:client-access, round 2
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

- story:model-tool-calling — it and the existing draft story:gateway-observability both change `crates/llm-gateway/src/relay.rs`, `checks/conformance/src/gateway.rs`, `docs/gateway.md`, `spec/domains/deployment.yaml` and `spec/domains/gateway.yaml`. Each of these is cited on at least one side. Neither body names the other, and no `depends_on` path orders them: both stop at `story:cold-start-hold`, and the two meet only below `story:client-qualification`. Fix: an ordering edge recording the shared files as its reason, or split the surface. — .engineering/planning/story/model-tool-calling.md:82-95, .engineering/planning/story/gateway-observability.md:110-139
- story:model-tool-calling — it and the existing draft story:live-runpod-wiring both change `spec/domains/deployment.yaml`. This is cited in both bodies: this story's default is recorded there, and live-runpod-wiring's ESS-first section adds `api_base_url` there. They also share `crates/llm-gateway-cli/src/serve.rs` (inferred on the live-runpod-wiring side). The Depends on and Neighbours sections name `story:pod-proxy-tls` but not live-runpod-wiring, and no path orders the pair. Fix: an ordering edge recording the shared files as its reason, or split the surface. — .engineering/planning/story/model-tool-calling.md:79-95, .engineering/planning/story/live-runpod-wiring.md:45
- story:pod-proxy-tls — it and the existing draft story:gateway-observability both change `crates/llm-gateway-cli/Cargo.toml` (TLS crate and `tracing`), `src/serve.rs`, `tests/binary.rs` and `spec/domains/deployment.yaml`. Every surface is inferred on the gateway-observability side, and `deployment.yaml` is cited on this side. Neither body names the other and no path orders them. Fix: an ordering edge recording the shared files as its reason, or split the surface. — .engineering/planning/story/pod-proxy-tls.md:67-75, .engineering/planning/story/gateway-observability.md:129-139
- story:public-model-listing — its `inventory()` and `start` change in `crates/llm-gateway-cli/src/serve.rs` (cited). Its two new unordered siblings, story:pod-proxy-tls and story:live-runpod-wiring, compose the binary in the same file (inferred on their side). The new edge to story:model-tool-calling does not order them: the three depend on it or its chain, and none depends on the others. Fix: ordering edges recording `serve.rs` as the reason, or split the surface. — .engineering/planning/story/public-model-listing.md:76-83, .engineering/planning/story/public-model-listing.md:109-111
- story:client-qualification — criteria 2 and 7 hold the story open until "a story named in the record" fixes a failed session and a rerun shows the tool call. The set's one story that handles a Claude Code failure is story:claude-code-wire (its D2 and D3 criteria), and it has `depends_on story:client-qualification`. So the fix lands after the state it must produce, and neither can be marked done first. Fix: the body says the fixing story is one outside this set that does not depend on it, or the pair is merged. — .engineering/planning/story/client-qualification.md:51-53, :62-63, .engineering/planning/story/claude-code-wire.md:10, :34

**What you read:** the 7 set items (model-tool-calling, pod-proxy-tls, client-qualification, claude-code-wire, loom-qualification, public-model-listing, hosted-endpoints), plus gateway-observability, live-runpod-wiring, gateway-deployment, usage-records, orphan-termination-obligations and review-result round 1. Commands: `aep plan artifact show` on each, `aep plan artifact waves --kind story --status draft`, `aep plan artifact graph`, `aep plan artifact relations`, and `AGENTS.md` "Planning and waves". I computed the transitive `depends_on` closure for every draft pair containing a set member and listed the pairs with no path and a shared scope file.
- **Surfaces:** 7 items cited, 0 inferred-only, 0 unplaced.
- **Acceptance reads traced to a producer in the set:** 12, with 12 recorded by a `depends_on` edge. The 13th, in the finding on client-qualification, is a read whose edge runs the opposite way.
- **Round 1:** findings 1 to 9 hold as resolved: the new edges give every pair path-ordered, and nothing in the set collides with story:target-fallback, story:gateway-translation, story:usage-records, story:hosted-endpoints or story:gateway-deployment unordered.
- **Merger-owned files:** `contracts/*`, `Cargo.lock` and `docs/llmgw-capability-matrix.md` appear in no typed scope, so they are not counted.

**What I could not establish:**
- Whether gateway-observability's O3 logging emits request lines and pod start and readiness times in UTC. Its body says only "logging follows `RUST_LOG`". The producer content criteria 1 and 5 of story:client-qualification read is unconfirmed, which belongs to the acceptance critic (out of my lane). The ordering edge exists.
- Whether story:pod-proxy-tls edits `docs/hosting.md`. Its scope lists it inferred, while story:orphan-termination-obligations cites it and no edge orders the pair. The body says nothing about editing the file, so I did not count it, and the design critic may judge the scope too wide.
- story:public-model-listing against story:gateway-observability on `server.rs` and `docs/gateway.md`: unordered, but gateway-observability's body names the pair and warns against one wave (`gateway-observability.md:93-102`). I did not count it as an unnamed collision.
- Every finding above is path-ordered by `aep plan artifact waves` (the colliding stories fall in different waves), but that order comes from scope and is not recorded in a body or edge. That is the same stance as round 1.

```findings
- file: .engineering/planning/story/model-tool-calling.md
  line: 82
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and the existing draft story:gateway-observability both change crates/llm-gateway/src/relay.rs, checks/conformance/src/gateway.rs, docs/gateway.md, spec/domains/deployment.yaml and spec/domains/gateway.yaml (each cited on at least one side, inferred on the other), neither body names the other and no depends_on path orders them; add an ordering edge recording the files or split the surface"
- file: .engineering/planning/story/model-tool-calling.md
  line: 79
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and the existing draft story:live-runpod-wiring both change spec/domains/deployment.yaml (cited in both bodies) and crates/llm-gateway-cli/src/serve.rs (inferred on the live-runpod-wiring side), the Depends on and Neighbours sections do not name live-runpod-wiring and no path orders them; add an ordering edge recording the files or split the surface"
- file: .engineering/planning/story/pod-proxy-tls.md
  line: 67
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it and the existing draft story:gateway-observability both change crates/llm-gateway-cli/Cargo.toml, src/serve.rs, tests/binary.rs and spec/domains/deployment.yaml with no ordering edge and no mention in either body; the surfaces are inferred on the gateway-observability side (deployment.yaml is cited on this side); add an ordering edge recording the files or split the surface"
- file: .engineering/planning/story/public-model-listing.md
  line: 109
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it changes inventory() and start in crates/llm-gateway-cli/src/serve.rs (cited) and its siblings story:pod-proxy-tls and story:live-runpod-wiring compose the binary in the same file (inferred on their side); the new edge to story:model-tool-calling does not order the three and no body names the others; add ordering edges recording serve.rs or split the surface"
- file: .engineering/planning/story/client-qualification.md
  line: 51
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance 2 and 7 hold the story open until a named story fixes a failed session and a rerun shows the tool call, and the only in-set story that handles a Claude Code failure, story:claude-code-wire, depends_on this story, so neither can be marked done first; the body must name a fixing story outside this set that does not depend on it, or the pair must be merged"
```
