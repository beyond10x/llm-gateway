---
format: aep.planning-md/3
id: story:call-id-and-usage-record
kind: story
status: implemented
title: Every model call has an id in its answer and a record the owner reads by it
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-25T22:37:22Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-25T22:37:22Z", actor: "human:operator", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-26T01:05:38Z", actor: "human:operator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":1}}}
---
## Outcome

Every model call has an id the client is told in the answer's head, and the owner reads that
call's record by the id: what was asked, how it ended, how long it took and the tokens the answer
reported.

## Why

A dispatcher books each run's model calls: tokens per call, per route. Today the gateway adds no
header to an answer, the `usage` log event carries no token field
(`crates/llm-gateway-cli/src/logging.rs`), and the token counters are sums on `GET /metrics`. A
client reads the upstream's own `usage` from the answer body, which a streamed chat answer carries
only when the client asked for it, and which a refused call does not have.

## Acceptance

1. Every answer to a request on a wire path that passed authentication carries
   `x-llm-gateway-call: <id>`: a relayed answer, a translated one, a refusal, and an answer on a
   client-credential route. An id is 1 to 64 of `A-Z a-z 0-9 -` and is not repeated after a
   restart of the gateway. A request that is shed or fails authentication gets none.
2. `GET /v1/calls/{id}` answers `200` with `{"format":"llm-gateway.call/1", …}`: `id`, `state`
   (`in-flight` or `ended`), `model`, `wire`, `disposition`, `outcome`, `refusal`, `status`,
   `target_status`, `input_tokens`, `output_tokens`, `cache_read_tokens`,
   `cache_creation_tokens`, `response_bytes`, `duration_ms`, `profile`, `principal`. A value the
   usage record does not carry is absent, never `0` and never an estimate. It takes the
   credential `GET /v1/launch` takes.
3. While an answer is still being written the record is `in-flight` and carries what is known;
   once the last byte is written it is `ended` and carries the usage record's values.
4. The gateway keeps the records of a published number of ended calls. An id it does not hold, or
   never gave out, is refused `404` with the new code `call-unknown`.
5. The `usage` log event carries `call` and the four token counters the record has.
6. A streamed chat answer whose client did not ask for usage has no token counters: the request
   body is relayed unchanged. `docs/gateway.md` says how a chat client asks for them.
7. The specification declares the header, the record and the code; scenarios cover acceptance 1,
   2 and 4; `docs/gateway.md`, the refusal reference and `CHANGELOG.md` say what is new.
8. Every test uses a fake upstream on loopback.

## Depends on

Nothing. `story:cost-per-call` adds the cost to this record.
