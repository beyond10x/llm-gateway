---
format: aep.planning-md/3
id: review-result:client-access-design-round-2
kind: review-result
status: active
title: Design critic, epic:client-access, round 2
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
approve

Round 1's three findings are closed: `story:public-model-listing` depends on `story:model-tool-calling`, `story:client-qualification` depends on `story:gateway-observability`, and `story:hosted-endpoints` now reuses `story:pod-proxy-tls`'s TLS client. The set has no cycle and no new defect.

What I read: 16 artifacts. These are the 6 epic and story items, `story:hosted-endpoints`, `story:public-model-listing`, the 4 blockers, round 1 (`review-result:client-access-design-round-1`) and its parallel-safety counterpart, and the upstream stories `story:cold-start-hold`, `story:gateway-observability` and `story:live-runpod-wiring`. I ran `aep plan artifact show` on each, plus `relations`, `graph` and `validate` (valid, 67 artifacts). I walked all 265 edges, outside the set as well, with a cycle check over `depends_on` and `blocks`, and found none. I also read `spec/domains/clients.yaml` and `crates/llm-gateway-cli/src/config.rs`. I traced 10 acceptance reads to a producer in the set, and an edge records all 10, directly or through a stated chain:
- the Claude Code fixture and the qualification record are read by `story:claude-code-wire`;
- the qualification record's alias is read by `story:loom-qualification`;
- the spend ceiling is read by both qualification stories;
- the log lines and per-call records are read by `story:client-qualification`;
- the tool-calling declaration is read by `story:public-model-listing` criterion 11;
- the test seam is read by `story:model-tool-calling` criterion 5, through `story:cold-start-hold`;
- the test-root seam is read by `story:hosted-endpoints` case 4.

What I could not establish:
- **Trade-off, not set as a finding.** The shared-file edge `story:hosted-endpoints` to `story:claude-code-wire` (`relay.rs`, `docs/gateway.md`) puts the operator-gated, paid live qualification in the path of `story:hosted-endpoints`, `story:target-fallback`, `story:usage-records` and `story:gateway-translation`. Those stories sit in another epic. `story:claude-code-wire` may end with no code change in criteria 2 to 4. The two options are keeping the order, as drafted with its reason written, or splitting the `relay.rs` and `docs/gateway.md` surface so the edge is not needed. I did not ask for the edge to be removed (`.engineering/planning/story/hosted-endpoints.md`, `## TLS and ordering`).
- **Unease.** `story:claude-code-wire` waits on all of `story:client-qualification`, so a Codex failure delays it. The two clients share one pod start and one cost ceiling, so I left it.
- **Out of my lane:**
  - Acceptance: `story:client-qualification` criterion 5 reads UTC pod start and ready times from log lines. `story:gateway-observability` lists counters, per-call records and `RUST_LOG` logging, and I found no line that states per-request or pod-start log lines.
  - Acceptance: `story:public-model-listing` says `max_model_len` in criterion 1 and `context_window` in criterion 9. `config.rs` has both names.
  - Acceptance: `story:claude-code-wire` criteria 2 to 4 are either/or.
  - Wording: `story:hosted-endpoints` `## TLS and ordering` says it supersedes lines of `## Scope` "below", but `## Scope` is above it.

```findings
[]
```
