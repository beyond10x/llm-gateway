---
format: aep.planning-md/3
id: story:keychain-references
kind: story
status: draft
title: A local gateway reads its keys from the OS keychain and manages Runpod pods without connectors
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
scope:
- confidence: cited
  path: crates/llm-gateway-cli/src/keychain.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/config.rs
- confidence: cited
  path: crates/llm-gateway-cli/src/routes.rs
- confidence: cited
  path: crates/llm-runpod/src/transport/rest.rs
- confidence: cited
  path: docs/design/runpod-clients.md
revision: 1
---
## Outcome

The deployment document may name the Runpod API key, a hosted endpoint's key, a model's vLLM key
and a route's secret as `keychain:<service>/<account>`. The binary reads each once at start from
the OS keychain (Secret Service on Linux, Keychain on macOS). A provider that names
`runpod_api_key` is driven over Runpod's REST API (`https://rest.runpod.io/v1`) directly, beside
`ConnectorsRunpod`, so a local gateway needs no `connectors` CLI.

## Acceptance

1. A key value never reaches a log line, an argument vector, a refusal text or the dashboard; a
   keychain entry that is missing or empty refuses the start naming the setting only.
2. The REST transport lists, creates, reads and terminates pods and reads spend and balance,
   tested against a loopback fixture of the API; no test reads a real keychain or calls Runpod.
3. `docs/design/runpod-clients.md` § 4 D1 and the routes document's `keychain:` rule say so.
