---
format: aep.planning-md/3
id: dependency-blocker:llm-usage-decoders
kind: dependency-blocker
status: cleared
title: llm exports no usage reader for all three wires (0.2.0)
relations:
- blocks: story:usage-records
revision: 4
transitions:
- {from: "open", to: "cleared", at: "2026-11-23T10:42:03Z", actor: "human:timo", revision: 4}
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

## Re-read at llm 0.5.0

Read 2026-10-08 from beyond10x/llm at tag `0.5.0` through the GitHub contents API, exports only:

- `b10x-llm-chat` exports `decode_completion` and `StreamProjection` (`src/lib.rs:26`); its
  incoming observation carries `usage` and `final_usage` (`src/incoming.rs:207-218, 356-358`).
- `b10x-llm-messages` exports `StreamDecoder`, `decode_message` and `decode_stream`
  (`src/lib.rs:12`); `Snapshot` is still `pub(crate)` (`src/usage.rs:7`).
- `b10x-llm-responses` exports `decode_stream` and `StreamDecoding` (`src/lib.rs:43`).

Whether each export yields token usage for streamed answers read piece by piece and for answers
that are not streamed was not read. The blocker stays open until that is read for all three.

## Answered by llm 0.25.0

Read 2026-11-23 from beyond10x/llm at tag `0.25.0` (commit `024d76e9`,
<https://github.com/beyond10x/llm/releases/tag/0.25.0>), `story:usage-reader` there:

- `llm_core::ReportedUsage { model: Option<Id>, usage: Usage, final_usage: bool }`.
- In each of `llm_responses`, `llm_messages` and `llm_chat`, module `usage_reader`:
  `UsageReader::new()`, `feed(&mut self, bytes: &[u8])`, `finish(self) -> ReportedUsage`, and
  `read_usage(body: &[u8]) -> ReportedUsage`. A body whose first non-whitespace byte is `{` is read
  as one JSON answer, anything else as an event stream; bytes may be fed in any chunks (tested at
  every split and byte by byte). It never fails: a cut answer reports what it read, with
  `final_usage` false.
- Each reader calls its wire's own decoder functions, so it reads what the decoder reads; a counter
  the answer did not report stays unknown, never zero.
- Not yet read: a Responses cache-write count (`stream::usage` drops it; llm review finding 8,
  being fixed in llm's next wave).

That meets "Cleared when" above, so this blocker is cleared. What `story:usage-records` still has
to do here, as read from this repository at `de2248a7`:

1. Move the llm pins (`tag = "0.24.0"` in `crates/llm-gateway-cli/Cargo.toml`,
   `crates/llm-runpod/Cargo.toml` and the other llm crates) to `0.25.0`.
2. Replace `UsageScanner` (`crates/llm-gateway/src/relay.rs:25, 1952, 1998, 2097, 2740, 2754`)
   with the reader of the request's wire: feed it the decoded body bytes where `scanner.feed` is
   called, then `finish()`.
3. In `crates/llm-gateway/src/server.rs` (`reported_model: None` at line 651,
   `reasoning_output_tokens: None` at line 656) map `reported_model` from `report.model`,
   `reasoning_output_tokens`, `cached_input_tokens` and `cache_creation_input_tokens` from
   `report.usage`; `final_usage` can mark partial counts.
4. Counters follow llm's `Usage`: `input_tokens` includes cache read and cache write. On Messages
   that total is unknown unless all three parts were reported, where the scanner records the raw
   `input_tokens`; the record and the dashboard have to accept that.
