---
format: aep.planning-md/3
id: story:hosted-credential-from-connectors
kind: story
status: draft
title: A hosted endpoint takes its credential from a connectors connection, not a key file
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:hosted-endpoints
revision: 1
---
## Outcome

A `[hosted.<alias>]` endpoint can take its credential from a connectors connection instead of a key
file. Started under `connectors connections launch --consumer …`, the binary reads the
connection's credential from descriptor 3 once at startup and sends it to the endpoint as its
`auth_kind` says (`bearer` or `api-key`), exactly as a `credential_file` credential is sent today.

## Why

Operator decision, 2026-10-29 (`approval-record:openai-hosted-qualification-no-runpod`): live
qualification runs against OpenAI's hosted API with a key held in connectors, in the custody the
Runpod key already has, so the gateway never holds a key file for it. Today every hosted endpoint
reads its key from a `0600` file (`credential_file`, `credential set`; `README.md` § hosted
endpoints).

## Acceptance

1. A hosted endpoint that names the connectors descriptor as its credential source, with the
   binary started under `connectors connections launch`, relays a request to an `https` hosted
   endpoint with the descriptor-3 credential in the header its `auth_kind` names; a test drives this
   with the `connectors-fixture` binary and a loopback `https` endpoint and asserts the header the
   endpoint received.
2. The same configuration with descriptor 3 absent, closed, empty or not a valid credential refuses
   the start with a named refusal before the listener opens, and no request is relayed.
3. A document that names both a `credential_file` and the connectors source for one endpoint is
   refused at start as a configuration error.
4. The credential is never written to disk and never appears in a log line, an error message, a
   metric or `GET /`; a test greps every output of a run that relayed a request, and of a refused
   start, for the credential value and finds none.
5. Endpoints that name `credential_file` behave exactly as before: their existing tests pass
   unchanged.
6. The refusal reference lists any new refusal code (`AGENTS.md` invariants), and `README.md`
   documents the new credential source beside `credential_file`.
7. The gate makes no paid call and reaches no hosted API (`AGENTS.md` invariants): every case runs
   against the fixture and a loopback endpoint.

## Depends on

`story:hosted-endpoints`: the hosted endpoint this changes. Blocked by
`upstream-blocker:connectors-openai-connector`: no connectors release can hold the OpenAI key this
source exists to carry.

## Not in scope

Writing anything in the connectors repository; changing how the Runpod key is reached.
