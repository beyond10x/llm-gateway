---
format: aep.planning-md/3
id: decision-blocker:usage-from-responses
kind: decision-blocker
status: open
title: Does the gateway parse target answers to read token usage?
relations:
- blocks: story:usage-records
revision: 2
---
## Question

Should the gateway read token usage out of the target's answers?

Today the relay hands the target's status, content type and body to the client unchanged and
reads none of them (`spec/domains/gateway.yaml`, `RelayObservation` step 9). llmgw had the same
rule as invariant 1: "Never grow request- or response-body interpretation beyond those two"
(llmgw `AGENTS.md`).

| Option | What it does | What it costs |
| --- | --- | --- |
| A | Parse a copy of every answer, event streams included, on all three wires, using llm's protocol crates taken by tag. The client's bytes stay unchanged. | The relay starts interpreting answers, three parsers sit on the response path, and llm becomes a dependency by tag. The gateway crate's dependency closure must stay empty, so the parse has to run behind a port the CLI crate implements. |
| B | Record only what needs no parsing: status, bytes, duration and disposition. | No token counts, so a call's cost cannot be attributed. |
| C | Record nothing here, and leave usage to llm clients' own cost ledger. | Claude Code and Codex are not llm clients, so their usage goes unrecorded. |

Recommendation at filing: A. It is the only option that serves O6 for the clients that reach the
gateway directly.

## Sources

`spec/domains/telemetry.yaml` (the `UNMAPPED` markers); llmgw `AGENTS.md` "Invariants";
`vision:portable-model-inference`.

## Cleared when

The operator picks A, B or C. The choice is written under a `## Decision` heading in this
artifact's body through `aep plan artifact body --section Decision`, naming who chose and when.
The `UNMAPPED` marker on reading usage in `spec/domains/telemetry.yaml` is rewritten as `DECIDED`
with the same answer in the same commit. Then `aep plan artifact move
decision-blocker:usage-from-responses --to cleared`.
