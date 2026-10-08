---
format: aep.planning-md/3
id: story:repo-operations
kind: story
status: implemented
title: 'AGENTS.md: in-tree builds, the objectives served, and the release process'
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: README.md
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T10:03:59Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-08T10:03:59Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-08T10:05:39Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`AGENTS.md` describes how this repository is built and released as it is now operated, and names
the objectives it serves:

1. **Builds use the tree's own `target/`.** The "Build into a shared directory" paragraph
   (`CARGO_TARGET_DIR=~/.cache/b10x-target/llm-gateway`) is replaced: every build writes the
   worktree's own `target/`, `CARGO_TARGET_DIR` is never set, and a tree ends with
   `worktree finish --discard-cache --archive <tree>`. `Taskfile.yml` sets no `CARGO_TARGET_DIR`
   today and stays so.
2. **`## Serves`** names Atlas objectives O1 (governed reach) and O5 (the generic agent platform),
   each with one line on what this repository contributes to it.
3. **`## Releases`** defines the release process before the first release is cut: workspace-wide
   version, bare tags (`0.1.0`), a source release with no assets (`publish = false`), the release
   commit on the wave's integration branch, the tag on the merge commit of the wave pull request,
   a GitHub Release, and the verification of tag, `Gate` and `Shared source gates` on that commit
   and of the Release before it is reported released.

`README.md` changes in the same commit wherever it says the opposite (status line, "No tag has
been cut").

## Acceptance

`grep -c 'b10x-target' AGENTS.md README.md Taskfile.yml` prints 0 for each file;
`grep -n '^## Serves' AGENTS.md` finds the section and it names O1 and O5;
`grep -n '^## Releases' AGENTS.md` finds a numbered procedure ending in the verification step; the
pull request's `Gate` and `Shared source gates` pass.

## Out of scope

Cutting the release itself; that is the wave's integration step, under the process this story
writes. No crate, specification or scenario changes.
