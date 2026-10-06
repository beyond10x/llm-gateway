---
format: aep.planning-md/3
id: review-result:replan-parallel-safety-round-1
kind: review-result
status: active
title: Parallel-safety critic, next three waves, round 1
relations:
- reviews: story:public-model-listing
- reviews: story:spec-diff-gate
- reviews: story:ess-0-53
- reviews: story:provider-key-files
- reviews: story:container-image
- reviews: story:gateway-spec-declarations
- reviews: story:hosting-spec-declarations
- reviews: story:runpod-production-transport
- reviews: story:hosting-lifecycle-declared
revision: 1
---
needs-revision

story:public-model-listing — it and story:provider-key-files both land on `crates/llm-gateway-cli/src/serve.rs` (inferred) and neither scope lists it. The listing needs `max_model_len`, which only the binary holds (`model.vllm.max_model_len`, `config.rs:449`). `TargetLimits` has no such field (`inventory.rs:106-111`). The literal in `serve.rs` `inventory()` and the `Gateway::bind` call in `start` must therefore change. The key files are read in that same module, as `owner_verifier` already does (`serve.rs:48`). Remedy: add an ordering edge recording `serve.rs` as the reason, or split the hand-off so the two stories no longer share the file. — `.engineering/planning/story/public-model-listing.md:29`

story:spec-diff-gate — `contracts/diff-acknowledgements.yaml` is a shared integration file, and neither the body nor `AGENTS.md:120-126` says so or says who owns it. The two wave-2 declaration stories delete or narrow declarations (`gateway-spec-declarations.md:44`, `hosting-spec-declarations.md:54`), which is the change this gate fails. An acknowledgement is bound to both endpoint digests, so it goes stale each time another spec-changing story merges. The same applies to every spec-editing story in wave 3. Remedy: add an ordering edge so the gate lands after the declaration stories, or have the merger own the file and list it in `AGENTS.md`. This is inferred from the acceptance text, because I did not run `ess verify diff`. — `.engineering/planning/story/spec-diff-gate.md:43`

story:ess-0-53 — its Ordering and `AGENTS.md` say it "runs alone, in a wave of its own", but `aep plan artifact waves` puts story:container-image in wave 1 beside it. I found no file overlap. container-image adds only `Dockerfile`, `docs/model-profiles.md` and a test, with no dependency, spec or scenario change. The body should say it runs alone among stories that edit the spec, scenarios, dependencies or the gate, or an edge should make the exclusivity real. — `.engineering/planning/story/ess-0-53.md:63`

story:provider-key-files — it reads the Runpod and vLLM key values at startup, and story:runpod-production-transport receives them "as values". Neither body names the carrier type or the file it lives in, and the two stories have no edge between them. Row B9 also changes how the pod gets its key (`crates/llm-runpod/src/request.rs:116-117`, `config.rs:75`), and no wave-3 scope lists those files. Inferred, and the weakest of the four. Remedy: name the hand-off type and its file in this body, or add an ordering edge. — `.engineering/planning/story/provider-key-files.md:49`

**Integration-file omission.** It hides no extra pairwise collision beyond finding 2. `contracts/ess-inputs.yaml` is a flat, sorted list of paths. The floor in `baseline.json` and `AGENTS.md:37` is a minimum count, so each story's own number is low rather than wrong. In every pair the merger has to regenerate or union by hand, but it can. I checked this against `AGENTS.md` and `checks/conformance/src/gate.rs`.

**Same authored file or symbol within a wave.** The only pair that lands on the same file is the one in finding 1.

- **Wave 1:** story:ess-0-53 and story:container-image touch disjoint files.
- **Wave 2:** gateway-spec-declarations (gateway.yaml, deployment.yaml, `docs/gateway.md`, `gateway.rs`) and hosting-spec-declarations (hosting.yaml, runpod.yaml, `docs/hosting.md`, `hosting.rs`, the `llm-provision` tests) are disjoint. spec-diff-gate edits only `gate.yml` and `Taskfile.yml`, and its only shared file is the acknowledgements file.
- **Wave 3:** hosting-lifecycle-declared (hosting.yaml, `hosting.rs`, `target.rs`) is disjoint from runpod-production-transport (runpod.yaml, `runpod.rs`, `llm-runpod/src/*`). The `deployment.yaml` collision in `waves` output between story:gateway-spec-declarations and story:provider-key-files is ordered by `depends_on`, and so are the `gateway.yaml`, `hosting.yaml`, `runpod.yaml` and `docs/hosting.md` overlaps across waves.

**What I read:** 10 stories via `cat` of `.engineering/planning/story/*.md`, `aep plan artifact waves --kind story --status draft`, `AGENTS.md` "Planning and waves" and "Gate", and the sources `serve.rs`, `config.rs`, `server.rs`, `inventory.rs`, `relay.rs`, `target.rs`, `Taskfile.yml`, `baseline.json`, `ess-inputs.yaml` and the matrix rows. Of the 9 stories in scope, 6 have a cited surface (ess-0-53, both spec-declarations, provider-key-files, public-model-listing, runpod-production-transport). 3 are inferred only (container-image, spec-diff-gate, hosting-lifecycle-declared). 0 are unplaced.

**What I could not establish:**
- Whether `ess verify diff` marks a deletion or an added enum type as breaking or unknown. I did not run it.
- Whether story:public-model-listing really cannot take `max_model_len` from `context_window`. The two are separate fields today.
- Out of my lane, and not setting the verdict: story:orphan-termination-obligations is excluded from the set, but it owns the `runpod.yaml` header, `docs/hosting.md` and `hosting.rs`. It would collide with the wave-2 and wave-3 hosting stories whenever it is scheduled, and the plan has no edge for it.
- Out of my lane: story:provider-key-files may change the document parser that story:container-image's profile test pins. Waves already order them, but the body does not say whether the new keys are optional.

```findings
- file: .engineering/planning/story/public-model-listing.md
  line: 29
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "this story and story:provider-key-files both land on crates/llm-gateway-cli/src/serve.rs (inferred): the listing needs max_model_len, which only the binary holds, so serve.rs inventory()/start must change, and key files are read in serve.rs like owner_secret_file; neither scope lists it and no edge orders them"
- file: .engineering/planning/story/spec-diff-gate.md
  line: 43
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "contracts/diff-acknowledgements.yaml is a digest-bound shared integration file that goes stale on every merge, and the body does not say who writes it or that the gate must land after the wave-2 declaration stories that delete declarations (inferred from acceptance text; ess verify diff not run)"
- file: .engineering/planning/story/ess-0-53.md
  line: 63
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the body and AGENTS.md:126 say this story runs alone in a wave of its own, but the derived wave 1 also holds story:container-image; no file overlap was found, so the exclusivity statement or an edge must change"
- file: .engineering/planning/story/provider-key-files.md
  line: 49
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "inferred: this story reads the key values at startup and story:runpod-production-transport receives them as values, but neither body names the carrier type or its file and no edge orders them; row B9 also touches crates/llm-runpod request.rs and config.rs, which no wave-3 scope lists"
```
