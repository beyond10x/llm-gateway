# Changelog

Every release is a source release tagged `<version>`; the crates are not published. The
[GitHub Releases](https://github.com/beyond10x/llm-gateway/releases) carry the same notes.
Nothing in any release is deployed or qualified against a live client.

## Unreleased

- Claude Code on the Messages wire: an effort of `max` (`output_config.effort` or
  `chat_template_kwargs.reasoning_effort`) now reaches the model as `xhigh`, as `high` already
  did, because the served Qwen3.8 chat template accepts only `xhigh`, `medium` and `low`.
  Adaptive `thinking`, `output_config.format` and the deprecated `output_format` are relayed
  unchanged, `anthropic-version` and `anthropic-beta` are dropped like every client header, and
  `HEAD /api/hello` stays refused; `docs/gateway.md` "The relay" says why for each, from vLLM
  v0.27.1's source and Claude Code's published gateway protocol. Settled from those sources and
  authored fixtures, not from a live session (`docs/verification/2026-10-15-claude-code-wire.md`).
- A hosted model may declare `reasoning_effort = "low" | "medium" | "xhigh"`. A Messages request
  that names no effort reaches its endpoint with `output_config.effort` set to it, so a server
  started by hand no longer thinks at its chat template's `xhigh` default when Claude Code sends
  no effort. A request that names one keeps it; the Chat and Responses wires and Runpod models,
  whose effort is the pod's own default, are unchanged.

## 0.7.0 (2026-10-14)

- The documentation site is published at <https://beyond10x.github.io/llm-gateway/>; README
  links it instead of the pages in the tree.
- Pod proxy TLS: the binary reaches a Runpod pod at `https://<pod id>-8000.proxy.runpod.net/v1/`
  over TLS (`rustls` on `ring`, in `b10x-llm-gateway-cli` only), the readiness probe and every
  relayed request alike, with the certificate verified against the Mozilla roots `webpki-roots`
  compiles in before any request byte is written. A certificate that does not verify is
  `upstream-failed`. No document key or option adds a root or disables verification. The
  start-time refusal of a document that declares a `connectors` provider is gone, so the binary
  now starts and relays to pods. `b10x-llm-runpod`'s probe takes its TLS connection from the
  embedding (`ConnectorsRunpod::set_tls`, `PodTls`). A binary started with a `connectors` provider
  and real Runpod credentials starts real, billed pods; no test reaches Runpod.
- `spec/domains/hosting.yaml` and `spec/domains/runpod.yaml` declare every rule the hosting
  contract promises: the hosting refusals with their codes and messages, the transition table,
  the evidence routes and listing completeness as typed enums, and the Runpod pool and
  declaration refusals with their codes. `b10x-llm-provision` publishes the evidence routes as
  `EvidenceKind`.
- A pod connection's 10 s connect bound covers the TLS handshake as one deadline; the handshake
  no longer waits under the 120 s read timeout. The `gateway-tls-seam` test program is built only
  with the test-only `test-seam` feature, so the release build writes `b10x-llm-gateway` and no
  program that trusts a root from its environment.
- Hosted endpoints: a model declared as `[hosted.<alias>]` is relayed to any server that speaks
  its wires at an `http` or `https` base URL (`base_url`, no path), as `bearer`, as `api-key` in
  the header its `credential_header` names, or `anonymous`, with the credential read once from
  `credential_file` through the trusted-file reader (`endpoint-credential:<rule>`). `https` uses
  the pod connection's TLS client and roots. A `bearer` or `api-key` endpoint at an `http` base
  URL whose host is not loopback (`127.0.0.0/8`, `::1`, `localhost`) is refused as
  `config:value`, so no credential is sent in cleartext to a remote host. The route inventory
  lists each hosted target's
  `auth_kind` and `billing_kind`, never its URL or credential. The gateway library gains
  `CredentialHeader` and `RelayTarget::credential_header` (`credential-header-bytes`, 64).

## 0.6.0 (2026-10-08)

- `GET /v1/models` and `GET /` answer without a credential and wake no pod. The listing is the
  OpenAI list shape, one entry per model with its `max_model_len` and wires. `GET /` answers the
  Codex, Claude Code or Loom settings for each model that declares the client's wire and parses
  tool calls, chosen by `User-Agent`, and counts one `llmgw_instruction_views_total`.
- A request head that is not UTF-8 is `request-malformed` on every route.
- `spec/domains/gateway.yaml` and `spec/domains/deployment.yaml` declare every rule the gateway
  contract and the README promise; the conformance suite holds 201 scenarios (was 191).
- A documentation site under `website/`, generated in part by the new `llm-gateway-docs` crate
  (the CLI, crate and refusal references and the status page), built by the `Documentation
  validation` workflow and deployed by the `Documentation site` workflow. `task check` and
  CI's `Gate` run `llm-gateway-docs generate --check`.
- The binary's clap definition moved to `llm_gateway_cli::cli::Cli`, so the CLI reference is
  generated from it. The command line is unchanged.

## 0.5.0 (2026-10-08)

- Runpod production transport: `ConnectorsRunpod` reaches Runpod only through the `connectors`
  CLI and its Runpod bundle (connectors v0.37.0): `pod.create`, `pods.list`, `pod.terminate`,
  each write with an approval proof for its exact input. An `unknown` create is resolved by one
  listing on the pod's name and request tag; a stale descriptor or an expired connection proof
  gets one bounded recovery.
- Model tool calling: each model declares `tool_calling = "parsed" | "absent"` (default
  `absent`). A non-empty `tools` array for an `absent` model is refused `tools-not-served` (400)
  before any target is asked for, so it starts no pod.
- Live Runpod wiring: `start_connected` composes the pool over `ConnectorsRunpod`. The shipped
  binary refuses a deployment document that declares a `connectors` provider until it can reach
  the pod's `https` endpoint, so this release cannot create a pod.
- Conformance: 191 scenarios (was 187).

## 0.4.0 (2026-10-08)

- Cold-start hold: a request for a model whose pod is starting is held up to the model's new
  `request_hold_seconds` (default `start_wait_seconds`), bound to that pod; past the budget it is
  answered `model-cold-start` (503, `retry-after: 30`). A pod that fails during the hold, or a
  hold that passes unbound, is `target-unavailable` for every request held on it. One cleanup
  pass before serving, then every 60 s.
- `RelayTargets::acquire` returns `Result<_, TargetRefusal>`; `b10x-llm-runpod` gains `Hold` and
  `RunpodPool::ensure_held`.
- Observability: `GET /metrics`, behind the owner credential, serves llmgw's 13 counters under
  their `llmgw_` names; one usage record per authenticated wire request; the binary logs through
  `tracing` with its level from `RUST_LOG`.
- Conformance: 187 scenarios (was 181).

## 0.3.0 (2026-10-08)

- ESS 0.56.0: CI installs it, the conformance runner builds on `ess-conformance` and
  `ess-primitives` 0.56.0, and the suite and schemas are regenerated with it.
- The relay sends each Runpod pod its model's vLLM key as `authorization: Bearer <key>`, and
  nothing of the client's request head. A failed pod is stopped and replaced, the graceful stop
  is never held by a request waiting for a pod, and `idle_timeout_minutes = 0` stops a pod at the
  first pass with no request in flight.
- A root `Dockerfile` builds a distroless, non-root image of `b10x-llm-gateway` on port 8080 with
  a source revision label. `docs/model-profiles.md` holds two proven model declarations and the
  weight-cache block.
- Conformance: 181 scenarios (was 179).

## 0.2.0 (2026-10-08)

- The deployment document takes an optional `vllm_api_key_file` per model, read once at startup
  through the trusted-file reader and held redacted; a broken file refuses with
  `vllm-api-key:<rule>`. `runpod_api_key_file` is refused: connectors holds the Runpod API key.
- `b10x-llm-runpod` takes the pod address from llm 0.5.0's Runpod provider description
  (`https://<pod id>-8000.proxy.runpod.net/v1/`). A pod whose id the description refuses gets no
  address and is never ready.

## 0.1.0 (2026-10-08)

- First release: the single-owner gateway library and the `b10x-llm-gateway` binary with the
  wire relay, the hosting lifecycle contract, the Runpod adapter over the in-process emulator,
  the ESS specification and its conformance suite (179 scenarios), and the design for using a
  Runpod-hosted model from Claude Code, Codex and Loom (`docs/design/runpod-clients.md`).
