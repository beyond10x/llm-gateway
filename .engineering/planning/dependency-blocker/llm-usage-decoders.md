---
format: aep.planning-md/3
id: dependency-blocker:llm-usage-decoders
kind: dependency-blocker
status: open
title: llm exports no usage reader for all three wires (0.2.0)
relations:
- blocks: story:usage-records
revision: 1
---
## What is missing

At llm 0.2.0, the latest release, no public API reads token usage out of an answer on all three
wires:

- `b10x-llm-chat`: `usage_of` is private (`incoming.rs:426`).
- `b10x-llm-messages`: `Snapshot` is `pub(crate)` (`usage.rs:7`).
- `b10x-llm-responses`: only `decode_stream(binding, &[Value])` is exported. It requires
  `stream: true` (`request.rs:168`), so it reads no answer that is not streamed, and it takes the
  whole stream as one slice.

All three crates also depend on `b10x-llm-http`, `b10x-llm-credentials`, `reqwest` and `tokio`.

Read by `story-scoper` from the local llm checkout at the 0.2.0 tree, 2026-10-06.

## Cleared when

An llm release exports a usage reader for each of the three wires, for streamed answers read
piece by piece and for answers that are not streamed. `story:usage-records` then names that tag.
The change belongs in beyond10x/llm, where it is filed and implemented.
