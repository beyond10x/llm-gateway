---
format: aep.planning-md/3
id: review-result:gateway-features-design-round-1
kind: review-result
status: active
title: Design critic, epic:gateway-features, round 1
relations:
- reviews: story:hosted-endpoints
- reviews: story:target-fallback
- reviews: story:usage-records
revision: 1
---
needs-revision
story:hosted-endpoints — its acceptance 2 needs the relay to put an upstream credential on the request, and the relay sends none; story:gateway-deployment already owns that for rows B8 and B9, and no edge records the order. Add `depends_on story:gateway-deployment`, or state in this body that it owns the relay's credential header and narrow B8 and B9 to the pod key. — `.engineering/planning/story/hosted-endpoints.md:39`; `docs/llmgw-capability-matrix.md:132` (B8: "No vLLM key is sent"); `docs/llmgw-capability-matrix.md:133` (B9); `crates/llm-gateway/src/relay.rs:582` (the request head is built with no authorization line; grep for `authorization` in relay.rs returns nothing); `aep plan artifact graph`
story:target-fallback — its Depends-on reason is that hosted-endpoints "adds the second kind of target a route can list", but hosted-endpoints declares one target per model, with a pod as "the other way" to serve it. No story in the set makes the document declare ordered targets or `fallback_enabled`. Today the CLI sets `fallback_enabled` to `false` and numbers positions by wire, not by alternative. Either this body owns the ordered-target and fallback-flag declaration (resolving the `UNMAPPED` marker beside the one hosted-endpoints owns), or the edge is dropped and fallback is shown at the target-source port only. — `.engineering/planning/story/target-fallback.md:52`; `.engineering/planning/story/hosted-endpoints.md:12`; `crates/llm-gateway-cli/src/serve.rs:91`; `crates/llm-gateway-cli/src/serve.rs:112`; `crates/llm-gateway-cli/src/config.rs:64`
story:usage-records — it holds two things. One is the per-call record that needs no parsing (`disposition`, `status`, `response_bytes`, `duration_ms`; acceptance 4). It is exactly what option B names, so the decision does not block it, and it carries the inputs of observability's O2 counters (requests, refusals, response bytes). The other is token extraction, which the decision does block. The whole story is blocked and the per-call observation has no stated owner. Move the no-parse record into `story:gateway-observability` (or its own story) and leave this story the token counters, or state in this body which story owns the per-call observation. — `.engineering/planning/story/usage-records.md:47`; `.engineering/planning/decision-blocker/usage-from-responses.md:23`; `docs/llmgw-capability-matrix.md:162` (O2); `spec/domains/telemetry.yaml:62`

What I read: 8 artifacts (the 5 in the set, `story:gateway-observability`, `story:gateway-translation`, `story:runpod-production-transport`) plus `story:gateway-deployment`, `story:cold-start-hold` and `epic:gateway`. I ran `aep plan artifact show`, `relations`, `graph` and `validate`; `validate` reported "valid". I walked all 52 declared edges in the store, including the 27 artifacts outside the set, and found no cycle. The `blocks`, `depends_on`, `decomposes` and `serves` edges are acyclic, and the only chain is hosted-endpoints to target-fallback, so the set is not serialised. Also read: `spec/domains/upstream.yaml`, `spec/domains/telemetry.yaml`, `crates/llm-gateway/src/relay.rs`, `crates/llm-gateway/src/inventory.rs`, `crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs` and `crates/llm-gateway/tests/dependency_boundary.rs`.

What I could not establish, and what is out of my lane (none of these set the verdict):
- Out of lane (acceptance): usage-records' Outcome says the counters join observability's series, but no acceptance case checks that.
- Out of lane (acceptance): hosted-endpoints' acceptance names no TLS or HTTPS case. Real hosted APIs need it, and the relay's `connect` returns a raw stream. I did not check whether the CLI can open TLS.
- Out of lane (scope): `spec/domains/telemetry.yaml:28` names `story:usage-records` as owner of the "does the gateway read usage" marker, while the blocker holds that decision.
- Out of lane (parallel-safety): target-fallback and usage-records both edit `relay.rs`, and hosted-endpoints shares the deployment-document grammar with `story:gateway-deployment`.
- I read `dependency_boundary.rs` only to confirm the forbidden crates (`tokio`, `reqwest`, and others). I did not run it.

```findings
- file: .engineering/planning/story/hosted-endpoints.md
  line: 39
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance 2 needs the relay to put an upstream credential on the request, and the relay sends none; story:gateway-deployment already owns that for rows B8 and B9, and no edge records the order. Add depends_on story:gateway-deployment, or state in this body that it owns the relay's credential header and narrow B8 and B9 to the pod key"
- file: .engineering/planning/story/target-fallback.md
  line: 52
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its Depends-on reason is that hosted-endpoints adds the second kind of target a route can list, but hosted-endpoints declares one target per model and no story in the set makes the document declare ordered targets or fallback_enabled (serve.rs:112 sets false, serve.rs:91 numbers positions by wire); either this body owns the ordered-target and fallback-flag declaration, or the edge is dropped and fallback is shown at the target-source port only"
- file: .engineering/planning/story/usage-records.md
  line: 47
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it holds two things: the per-call record that needs no parsing (disposition, status, response_bytes, duration_ms; acceptance 4), which is option B's content and the inputs of observability's O2 counters, and token extraction, which the decision blocks; move the no-parse record into story:gateway-observability or its own story, or state which story owns the per-call observation"
```
