# Hand-over: llm-gateway session, 2026-10-06

This session wrote the gateway feature research, hardened the ESS specification and re-planned and
re-scoped the AEP store. It wrote no implementation code.

## State

| Item | State |
| --- | --- |
| `main` | `9c4ef18`, pushed by `b10x-bot[bot]`. The two commits above `00895dd` are `a11ef83` (research, planned domains, `epic:gateway-features`) and `9c4ef18` (hardening, replan, typed scope). This hand-over is the commit after them. |
| CI on `9c4ef18` | "Shared source gates" passed. "Gate" was still running when this was written. |
| Local gate | `RUSTUP_TOOLCHAIN=1.98.0 task check` exited 0 on `9c4ef18`: 294 cargo tests passed, conformance 179 of 179 on three runs |
| Branches, pull requests | None besides `main`; the repository moves `main` by fast-forward |
| Unpushed commits | None |
| Worktrees | `llm-gateway-plan` is finished with this hand-over. `llm-gateway-spec` was finished earlier and holds nothing. |
| Primary checkout `~/beyond10x/llm-gateway` | Clean, at `76059a3`; fast-forward it before use |
| Filed upstream | https://github.com/beyond10x/ess/issues/469 (`ess verify diff` reports an added domain as `unknown`) |
| Build cache | `~/.cache/b10x-target/llm-gateway`, 1.2G, the repository's shared target directory |

## What changed in the plan

- `epic:gateway-features` holds three stories from the 2026-10-06 research, and its body carries
  the sources and the exclusions: `story:hosted-endpoints`, `story:target-fallback` and
  `story:usage-records`.
- `epic:spec-hardening` follows `docs/verification/spec-hardening.md` and the two review tables
  beside it. Its stories are `story:gateway-spec-declarations`, `story:hosting-spec-declarations`,
  `story:spec-diff-gate` and `story:hosting-lifecycle-declared`.
- `story:gateway-deployment` is now row B8 only. Its other rows moved to `story:provider-key-files`
  (K7, B9), `story:public-model-listing` (R1, R5) and `story:container-image` (D1, D3).
  `story:live-runpod-wiring` is new.
- Every draft story has typed scope. `AGENTS.md` "Planning and waves" names the integration files
  the wave merger owns.

Decisions recorded in the specification on 2026-10-06:

- The gateway parses a copy of each answer for token usage. The operator chose option A, and
  `decision-blocker:usage-from-responses` is cleared.
- Any endpoint that speaks one of the three wires is a hosted endpoint, with no vendor-specific
  code.
- A 429 falls back to the next target.
- An `api-key` endpoint declares the header that carries its credential.
- The vLLM key is read from a file, not derived from the Runpod key.

## Next step

Wave 1 through `aep:implementing`: `story:ess-0-53` and `story:container-image`. The waves
`aep plan artifact waves --kind story --status draft` derives after that:

| Wave | Stories |
| --- | --- |
| 2 | `story:gateway-spec-declarations`, `story:hosting-spec-declarations` |
| 3 | `story:spec-diff-gate` |
| 4 | `story:provider-key-files`, `story:runpod-production-transport` |
| 5 | `story:gateway-deployment`, `story:hosting-lifecycle-declared` |

## Open

- `story:orphan-termination-obligations` has no acceptance yet, so no wave takes it.
- `story:usage-records` is blocked by `dependency-blocker:llm-usage-decoders`. llm 0.2.0 exposes
  no usage reader for all three wires. Nothing is filed in beyond10x/llm yet.
- Escalated planning findings: `story:runpod-production-transport` and `story:gateway-translation`
  name no concrete file surface of their own beyond the scoper's inference
  (`review-result:gateway-features-parallel-safety-round-1` and `-round-2`).
- The llmgw cutover (llm `story:llmgw-retirement`) waits on the parity stories in waves 4 to 9.
