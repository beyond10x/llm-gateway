---
format: aep.planning-md/3
id: review-result:critic-parallel-w08-round-1
kind: review-result
status: active
title: Parallel-safety critic, wave 2026-10-15-w01 new stories, round 1
relations:
- reviews: story:effort-default
- reviews: story:max-tokens-clamp
revision: 1
---
needs-revision

story:effort-default — both this and story:max-tokens-clamp land on `crates/llm-gateway/src/body.rs`, `relay.rs`, `tests/wire_relay.rs`, `docs/gateway.md` "The relay", `spec/domains/gateway.yaml` (the `Relay` command), `contracts/gateway/scenarios` and `CHANGELOG.md` (cited, both scope lists and both "Depends on" sections), and neither body names the other or records the shared files, so the plan reads as two parallel trees when it is not — .engineering/planning/story/effort-default.md:83, .engineering/planning/story/max-tokens-clamp.md:84

The remedies, not chosen between:
- (a) A `depends_on` edge between the two whose stated reason is the shared files.
- (b) Splitting the surface so they stop sharing it. For example, each story gets its own `Relay`-command section, its own step number in `docs/gateway.md`, and its own scenario and `wire_relay` module.

Details a person deciding this should have:
- **`docs/gateway.md` numbering:** effort-default edits "step 6" (effort-default.md:78), while max-tokens-clamp adds a new step to "The relay" (max-tokens-clamp.md:78). Two trees would renumber the same list, and they would clash on `CHANGELOG.md` too.
- **`body.rs` rewrite path:** effort-default changes the rewrite that maps effort. It also inserts an effort key into `output_config` or adds the object (effort-default.md:68-71). max-tokens-clamp rewrites `max_tokens` in place in the same body (max-tokens-clamp.md:70-72). Both extend the single `Analysis`/`rewrite` pair in `crates/llm-gateway/src/body.rs`, which records byte spans, so the edits are not independent.
- **`relay.rs` (cited for the clamp, inferred for effort-default):** the clamp's count must run on "the rewritten body" (max-tokens-clamp.md:68). That body includes effort-default's insertion, so the count request's content depends on whether effort-default has landed.
- **`crates/llm-gateway-cli/src/hosted.rs` (inferred for both):** the new `reasoning_effort` declaration and the `context_window` handling both plausibly touch the hosted-model builder around line 377. This one is inferred from the nouns, not cited.

Not a defect: merging shared files by hand is a legitimate choice. The bodies should say so, and a person should decide.

**What I read:** 3 artifacts (the two drafted stories and `story:claude-code-wire`). Commands: `aep plan artifact show` on each, `aep plan artifact graph`, `aep plan artifact relations`, `git grep` for `context_window`, `max_model_len` and `reasoning_effort` in the tree, and `body.rs` and `hosted.rs` read directly.

**What I could not establish:**
- **Surface counts:** 2 items placed with cited surfaces, 0 placed only by inference, 0 unplaced. Individual surfaces inside them marked inferred (`relay.rs` and `hosted.rs` for effort-default, `hosted.rs`, `serve.rs`, `metrics.rs` and `tests/gateway.rs` for the clamp) are noted above where they matter.
- **Acceptance reads:** 0 traced to a producer inside the two-item set. 3 acceptance reads trace to `story:claude-code-wire`: the Claude Code fixture, the `max` mapping and step 6. That story is active and outside the pair, and each is recorded by a `depends_on` edge on both stories. The clamp reads `max_model_len` and `context_window`, which already exist in `config.rs` and `hosted.rs`, so there is no producer in the set.
- **`serve.rs` and `metrics.rs`:** I did not check whether the clamp's counters collide with anything in effort-default, which names neither file.
- **Out of lane:** `story:claude-code-wire`'s "Neighbours" section lists the later stories that touch `relay.rs`, `body.rs` and `docs/gateway.md`, but not these two. That is a gap in a different artifact and outside the set I was given.

```findings
- file: .engineering/planning/story/effort-default.md
  line: 83
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "both this and story:max-tokens-clamp land on crates/llm-gateway/src/body.rs, relay.rs, tests/wire_relay.rs, docs/gateway.md 'The relay', spec/domains/gateway.yaml Relay command, contracts/gateway/scenarios and CHANGELOG.md (cited, both bodies; relay.rs is inferred on this side) and neither body names the other or records the shared files; remedies are an ordering edge recording the shared files as its reason, or splitting the surface so they no longer share it (cited: .engineering/planning/story/max-tokens-clamp.md:84)"
```
