---
format: aep.planning-md/3
id: story:docs-site
kind: story
status: active
title: llm-gateway has its own documentation site in the shared look and feel
relations:
- decomposes: epic:gateway
- serves: vision:portable-model-inference
- depends_on: story:public-model-listing
scope:
- confidence: inferred
  path: .github/workflows/b10x-docs-site.yml
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: .github/workflows/pages.yml
- confidence: inferred
  path: .gitignore
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: Taskfile.yml
- confidence: inferred
  path: crates/llm-gateway-docs
- confidence: inferred
  path: website
revision: 12
transitions:
- {from: "draft", to: "proposed", at: "2026-10-08T20:01:24Z", actor: "human:timo", revision: 11}
- {from: "proposed", to: "active", at: "2026-10-08T20:01:24Z", actor: "human:timo", revision: 12}
---
## Outcome

llm-gateway has its own documentation site at `https://beyond10x.github.io/llm-gateway/`, built
from `website/` in the shared look and feel of the other independent beyond10x sites, and
`README.md`, `AGENTS.md` and the site say the same thing about the same crates and commands.

## Why

The repository has no site: `https://beyond10x.github.io/llm-gateway/` answered HTTP 404 on
2026-10-08, the GitHub repository's homepage field is empty, and the README links the in-tree
`docs/` pages instead. The workspace `docs` skill (`~/beyond10x/.agents/skills/docs/SKILL.md`)
defines the page set, the generated pages and the two workflows every independent site shares.

## Acceptance

1. `website/` builds with `npm --prefix website ci && npm --prefix website run build` (exit 0),
   with `@beyond10x/docs-system` pinned to the commit the other independent sites pin.
2. The page set of the docs skill § 2 exists under `website/docs/`: `index.md`,
   `getting-started.md`, `concepts/`, `guides/`, `reference/` and the status page, each with the
   front matter the skill names. Every command on a page was run in the tree and its output pasted
   from that run. Draft stories appear as planned, never as shipped.
3. A crate `crates/llm-gateway-docs` generates the CLI reference from the binary's clap
   definitions, the crate list from `cargo metadata` and `data/status.json`; every `shipped`
   status item names the test that holds it. `generate --check` is a step of `task check` and
   fails on drift; `provenance --site <dir> --commit <sha>` binds a built site to its commit.
4. `.github/workflows/pages.yml` (`Documentation validation`) and
   `.github/workflows/b10x-docs-site.yml` (`Documentation site`) follow the skill's Stage A.
5. `README.md` and `AGENTS.md` are consistent with the site and with each other: README links the
   site on its first screen and the pages it hands off to; AGENTS.md names the docs crate, its
   check and the site build in the gate.
6. After the merge to `main`, both workflows run green, the three `curl` checks of the skill's
   Stage D pass, and the repository's homepage field is the site's URL.

Steps of the skill's Stage C that change other repositories are requested from their owners; they
are not part of this story.

## Scope (inferred)

`website/**` (new), `crates/llm-gateway-docs/**` (new), `Cargo.toml` (workspace member),
`Taskfile.yml`, `.github/workflows/pages.yml` (new), `.github/workflows/b10x-docs-site.yml` (new),
`.github/workflows/gate.yml`, `CHANGELOG.md` (new), `README.md`, `AGENTS.md`, `.gitignore`.

## Depends on

- `story:public-model-listing`: the site documents the routes it adds, and both change
  `README.md` and `AGENTS.md`.
