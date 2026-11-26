---
format: aep.planning-md/3
id: story:call-bytes-tap
kind: story
status: archived
title: A bounded copy of a call's request and answer bytes, off unless declared
relations:
- decomposes: epic:interaction-records
- serves: vision:portable-model-inference
revision: 2
transitions:
- {from: "draft", to: "archived", at: "2026-11-26T02:19:37Z", actor: "human:operator", revision: 2}
---
## Outcome

An embedding that declares it receives, with a call's finished-call value, a bounded copy of the
call's request body and of its answer as the client received it: the body of an answer that was
not streamed, or the `data:` payload of each server-sent event of one that was. Without the
declaration the gateway copies nothing.

## Why

sessionspec's wire lift reads the request and the answer (`WireCall { request, answer:
Answer::Body | Answer::Events }`) to find the agent chain, prompts, tool calls and the message id.
The gateway buffers the request body (at most 32 MiB, `crates/llm-gateway/src/relay.rs:45`) but
relays the answer as it streams and keeps none of it; the usage scanner reads the usage object only
(`crates/llm-gateway/src/usage.rs:155`).

## Acceptance

1. A declaration on the relay (`with_call_bytes(<bound>)`) turns the copy on, with one byte bound
   per call for the request and the answer together; the default is off and copies nothing.
2. A call whose bytes exceed the bound hands no bytes and says it was over the bound; the relay's
   answer to the client is unchanged by the copy, byte for byte, on every wire and path.
3. For a streamed answer the copy is the ordered list of `data:` payloads as the client received
   them, events split across reads included; a test splits each event of a recorded stream at
   every byte offset.
4. A translated call's copy is the inbound wire's request and the answer as translated for the
   client; a count-tokens call and a refusal hand no answer bytes.
5. `spec/domains/telemetry.yaml` declares the copy, its bound and the over-bound mark; a document
   names its cost: one more copy of the answer per call while the declaration is on.
6. A test measures the relay's time per call with and without the declaration on a recorded
   1 MiB streamed answer; both numbers are in the pull request.
