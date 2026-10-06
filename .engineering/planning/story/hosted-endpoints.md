---
format: aep.planning-md/3
id: story:hosted-endpoints
kind: story
status: draft
title: A relayed model can be served by a hosted endpoint, not only a Runpod pod
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- depends_on: story:gateway-deployment
- depends_on: story:cold-start-hold
- depends_on: story:gateway-observability
revision: 4
---
## Outcome

A relayed model can be served by a hosted endpoint declared in the deployment document: any
server that speaks the model's wire at an `http` or `https` base URL, authenticated with a
credential read from a file through the trusted-file reader. A Runpod pod remains the other way
to serve a model.

## Why

`runpod-vllm` is the only provider kind the deployment document admits
(`llm-gateway.deployment.ProviderKind`). Every gateway in the 2026-10-06 research reaches hosted
APIs as well (`epic:gateway-features`, row "Hosted provider endpoints").

## ESS first

`llm-gateway.upstream.HostedEndpoint` in `spec/domains/upstream.yaml` is marked `PLANNED`. This
story adds its observation entity and scenarios, and settles the marker `UNMAPPED: how the closed
deployment document declares an endpoint` in the specification before any code.

## Acceptance

Cases 2, 3, 4 and 7 are `Relay` conformance scenarios, and case 1 is an `Exchange` inspection
scenario. Cases 5 and 6 are startup refusals, and they are proved the way the binary's other
refusals are: as row-named process tests in `crates/llm-gateway-cli/tests/`, because the
conformance targets do not model files or processes (`spec/domains/deployment.yaml`,
`ESS-LIMIT`). Every case runs against a loopback fixture endpoint, and none makes a paid call.

1. A deployment document that declares a hosted endpoint for a model starts. `GET /v1/routes`
   lists that model's target with its `auth_kind` and `billing_kind`. Neither the base URL nor
   any byte of the credential file appears in any response, which the test checks by searching
   every byte written.
2. A relayed request for that model reaches the fixture at the declared base URL, on the same
   path. A `bearer` endpoint receives `authorization: Bearer <credential>`. An `api-key`
   endpoint receives the credential as the value of the header its `credential_header` names,
   and no `authorization` header. An `anonymous` endpoint receives neither.
3. The owner's bearer credential is absent from every byte any fixture receives.
4. An `https` endpoint is reached over TLS. A fixture whose certificate the gateway does not
   trust ends the request `upstream-failed`, and no request byte reaches it.
5. A credential file that breaks a trusted-file rule refuses startup with
   `endpoint-credential:<rule>`, using the rules `owner_secret_file` follows
   (`llm-gateway.deployment.StartupRefusal`).
6. The document is refused as `config:value` in two cases: an endpoint whose `auth_kind` is not
   `anonymous` but which declares no credential file, and an `api-key` endpoint that names no
   `credential_header`.
7. A model served by a Runpod pod behaves exactly as before: the `Relay` scenarios in
   `contracts/gateway/scenarios/` still pass.

## Depends on

- `story:gateway-deployment`. It adds the relay's upstream credential header (rows B8, B9: the
  vLLM key sent to the pod). This story reuses that header for hosted endpoints rather than adding
  a second one. Both stories change `crates/llm-gateway-cli/src/config.rs` (`ProviderKind`,
  `Provider`) and `crates/llm-gateway-cli/src/serve.rs` (the target source).
- `story:cold-start-hold`. It edits the same deployment document (row K28) and the same
  `RelayTargets` port (`crates/llm-gateway/src/relay.rs:86-94`).
- `story:gateway-observability`. It adds the per-call record and its conformance observation in
  `crates/llm-gateway/src/relay.rs` and `checks/conformance`, which this story's scenarios also
  change.

## Scope (inferred)

`crates/llm-gateway-cli/src/config.rs`, `crates/llm-gateway-cli/src/serve.rs`, the relay's
connection setup in `crates/llm-gateway-cli`, `spec/domains/deployment.yaml`,
`spec/domains/upstream.yaml`, `contracts/gateway/scenarios`, `checks/conformance`,
`docs/gateway.md`, `README.md`.
