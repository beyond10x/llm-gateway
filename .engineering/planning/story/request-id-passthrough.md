---
format: aep.planning-md/3
id: story:request-id-passthrough
kind: story
status: draft
title: A request id reaches the target and the gateway's log line
relations:
- decomposes: epic:gateway-features
revision: 1
---
## Outcome

A request's `x-request-id` reaches the target and the gateway's log line for that request, so a
call can be followed from client to model server.

## Acceptance

1. A client's `x-request-id` (one header, printable ASCII, at most 128 bytes) is forwarded to the
   target unchanged, an exception to "every client header is dropped" stated in `docs/gateway.md`
   step 7; a malformed or repeated one is replaced.
2. A request without one gets a generated id; the id is in the usage record, its `tracing` event,
   and the answer's `x-request-id` header.
3. Specification, a `Relay` scenario and a `wire_relay` test cover forwarded, generated and
   replaced ids on all three wires.
