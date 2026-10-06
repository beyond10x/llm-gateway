---
format: aep.planning-md/3
id: review-result:replan-parallel-safety-round-2
kind: review-result
status: active
title: Parallel-safety critic, next three waves, round 2
relations:
- reviews: story:spec-diff-gate
- reviews: story:hosting-lifecycle-declared
- reviews: story:provider-key-files
- reviews: story:runpod-production-transport
- reviews: story:ess-0-53
- reviews: story:container-image
- reviews: story:gateway-spec-declarations
- reviews: story:hosting-spec-declarations
revision: 1
---
needs-revision

spec-diff-gate — it shares wave 3 with three stories that edit `spec/`, add scenarios or add a dependency (`story:provider-key-files`, `story:hosting-lifecycle-declared`, `story:runpod-production-transport`). `AGENTS.md` says the wave of a story whose purpose is an integration file or the gate holds no such story, and this story's own acceptance 6 makes `contracts/diff-acknowledgements.yaml` one. No edge orders them, and the body's merger-rewrites-the-acknowledgements paragraph does not say why the rule is waived. Whether the new gate fails those three stories' spec changes is inferred. Both remedies are open: an ordering edge between this story and each of the three, with the shared gate and acknowledgements file as the reason, or a body line recording the waiver. — .engineering/planning/story/spec-diff-gate.md:52 (rule at `AGENTS.md:126-127`; sibling scopes at provider-key-files.md:25, runpod-production-transport.md:33, hosting-lifecycle-declared.md:17)

hosting-lifecycle-declared — its scope lists the shared dispatcher `checks/conformance/src/target.rs` (inferred) and its body never says whether `Observed` or `execute_command` changes. Acceptance 2 needs per-transition command outcomes, but `target.rs:104` hard-codes the outcome `"observed"` for every command, and `Observed` is built in `runpod.rs:59`, `hosting.rs:50` and `gateway.rs:62,861`. A change there lands on `checks/conformance/src/runpod.rs` (`story:runpod-production-transport`, same wave, no edge) and on `checks/conformance/src/gateway.rs` (`story:gateway-spec-declarations`, no edge to this story). Both remedies are open: an edge on the shared conformance files, or a body statement that `target.rs` and `Observed` stay unchanged. — .engineering/planning/story/hosting-lifecycle-declared.md:39 (scope at :14; `checks/conformance/src/target.rs:39-44,104`)

**What I read:** 8 story bodies and typed scopes, plus `AGENTS.md`, `Taskfile.yml`, `contracts/baseline.json`, `contracts/ess-inputs.yaml`, `checks/conformance/src/{target,gate}.rs` and the hosting, gateway and runpod specs. Commands: `aep plan artifact waves --kind story --status draft`, `aep plan artifact graph`, `git grep`, `cat`. I did not run `aep plan artifact scope` because it writes, so scopes were read from each story's frontmatter. No review-result artifact was read. Surfaces: 5 stories have cited surfaces (`story:ess-0-53`, `story:gateway-spec-declarations`, `story:hosting-spec-declarations`, `story:provider-key-files`, `story:runpod-production-transport`), 3 are inferred only (`story:container-image`, `story:hosting-lifecycle-declared`, `story:spec-diff-gate`), 0 are unplaceable.

**Checked and clear:**
- **Wave 1:** `story:ess-0-53` and `story:container-image` share no authored file, and the second adds no spec edit, scenario, dependency or gate change.
- **Wave 2:** the two stories' typed scopes are disjoint (gateway and deployment files versus hosting and runpod files, different review files, different crates). The shared `contracts/suite.json`, `contracts/schema/`, `contracts/ess-inputs.yaml` and `contracts/baseline.json` are regenerated or merged by hand under the "Planning and waves" rule. The omitted integration files hide no conflict in this wave.
- **Wave 3 pairs:** `story:provider-key-files` against the other three, and `story:hosting-lifecycle-declared` against `story:runpod-production-transport`, share no authored file.
- **Cross-wave overlaps:** every other shared file is ordered by an edge, direct or through `story:ess-0-53`. The files are `spec/domains/deployment.yaml`, `spec/domains/hosting.yaml`, `spec/domains/runpod.yaml`, `docs/hosting.md`, `checks/conformance/src/hosting.rs`, `.github/workflows/gate.yml` and `Cargo.lock`. `AGENTS.md` and `README.md` collisions fall on different sections, so they are hand merges.

**What I could not establish:**
- Where `story:container-image` puts the test that parses the Dockerfile (acceptance 1). Its scope lists only `crates/llm-gateway-cli/tests/profiles.rs`, which could share `crates/llm-gateway-cli/tests/binary.rs` or `tests/support/` with `story:provider-key-files`. There is no edge between the two.
- Whether `story:runpod-production-transport` changes `RunpodModel` or `VllmSettings` (`crates/llm-runpod/src/config.rs:45,61`) to carry the vLLM key. `crates/llm-gateway-cli/src/config.rs:11` (`story:provider-key-files`) builds on `VllmSettings`. The body leaves the trait change open.
- Out of my lane: the checkability of acceptance 4 on `story:hosting-lifecycle-declared` (the "179 scenarios" count), and the inconsistency between "a story writes its own acknowledgement" and "the wave merger rewrites it" in `story:spec-diff-gate`.

```findings
- file: .engineering/planning/story/spec-diff-gate.md
  line: 52
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "wave 3 holds story:provider-key-files, story:hosting-lifecycle-declared and story:runpod-production-transport, which edit spec/, add scenarios or add a dependency, in the same wave as this gate-changing story whose acceptance 6 makes contracts/diff-acknowledgements.yaml an integration file; AGENTS.md:126-127 bars that mix, no edge orders them, and the body does not say why the rule is waived (that the gate fails their spec changes is inferred); remedies are an ordering edge naming the shared gate and acknowledgements file, or a recorded waiver"
- file: .engineering/planning/story/hosting-lifecycle-declared.md
  line: 39
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "inferred: the scope lists checks/conformance/src/target.rs and acceptance 2 needs per-transition outcomes that target.rs:104 hard-codes as 'observed', so Observed or execute_command may change, which every domain module constructs (runpod.rs:59, hosting.rs:50, gateway.rs:62,861); runpod.rs is in story:runpod-production-transport (same wave, no edge) and gateway.rs in story:gateway-spec-declarations (no edge), and the body does not say target.rs stays unchanged; remedies are an edge recording the shared conformance files, or a body statement that Observed and execute_command are unchanged"
```
