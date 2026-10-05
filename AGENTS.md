# AGENTS.md — llm-gateway

What this repository is for is in [README.md](README.md); this file is what an agent changing it must
know.

## State

The repository holds no code yet. The gateway, hosting and provisioning crates move here from
beyond10x/llm under llm `story:serving-extraction` (llm `epic:serving-split`), with their history,
tests, ESS sources and open plan. Until then, change them in beyond10x/llm.

## Rules

- Anything that runs is Rust, with clap derive for command lines.
- Every commit and push is `b10x-bot[bot]`'s through `b10x-gates bot`; every GitHub write goes
  through `b10x-gates api`.
- Use a managed worktree for changes.
- No `/home/<name>/` path literals anywhere: common Gates personal-paths has no allowance.
