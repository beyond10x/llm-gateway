---
format: aep.planning-md/3
id: story:runpod-key-file
kind: story
status: draft
title: The Runpod account key is read from a file the owner fills
relations:
- decomposes: epic:hosting
- serves: vision:portable-model-inference
revision: 1
---
## Outcome

The Runpod account key can be read from a file: `[providers.<name>] api_key_file` names a file
the owner fills, read once at start under the trusted-file rules, as `vllm_api_key_file` is.

## Why

The owner keeps the Runpod API key in one file on the host that the gateway reads; today
`[providers.<name>] api_key` takes a `keychain:` reference only (story:keychain-references), so a
key in a file cannot be used without copying it into the OS keychain.

## Acceptance

1. `[providers.<name>] api_key_file`, a path, declared in `spec/domains/deployment.yaml`. It is
   refused beside `api_key` and beside `connectors` (`config:value` naming
   `providers.<name>.api_key_file`), and counts as `api_key` for "at most one provider declares
   `connectors` or `api_key`".
2. The file is read once at start under the trusted-file rules of `vllm_api_key_file` (opened
   without following a final symlink, mode & 0o077 refused, size bound, token rule); a refusal is
   `endpoint-credential:<rule>` with a message starting `providers.<name>.api_key_file:`, never
   naming the path or the value.
3. A test per rule against a fixture file, one that a key read from the file reaches the fake
   Runpod REST API as `Authorization: Bearer <key>`, and one that a missing file refuses the start
   with no request sent.
4. `docs/gateway.md` (deployment document) and `docs/runpod-routes.md` ("The account key") name
   the key; the README's example keeps `api_key`.
