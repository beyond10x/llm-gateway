---
format: aep.planning-md/3
id: story:bedrock-hosted-target
kind: story
status: implemented
title: A hosted model on Amazon Bedrock, translated and signed with container credentials
summary: 'kind = bedrock in [hosted.<alias>]: llm 0.11.0''s Bedrock client behind the translator, credentials from the container credentials endpoint'
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: README.md
- confidence: inferred
  path: contracts
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/bedrock.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/config.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/hosted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/relaying.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/translate.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/trusted.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/bedrock.rs
- confidence: inferred
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: crates/llm-gateway/src/relay.rs
- confidence: inferred
  path: crates/llm-runpod/Cargo.toml
- confidence: inferred
  path: docs/gateway.md
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: spec/domains/upstream.yaml
- confidence: inferred
  path: website/docs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T22:09:36Z", actor: "human:timo", revision: 4, executor: "agent:llm-session", correlation: "wave-2026-10-23-w01"}
- {from: "proposed", to: "active", at: "2026-10-22T22:09:37Z", actor: "human:timo", revision: 5, executor: "agent:llm-session", correlation: "wave-2026-10-23-w01"}
- {from: "active", to: "implemented", at: "2026-10-22T23:25:48Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1}}, executor: "agent:llm-session"}
---
## Outcome

A `[hosted.<alias>]` entry with `kind = "bedrock"` serves a model on Amazon Bedrock Runtime: the
gateway decodes the client's chat, responses or messages request onto llm's neutral turn, sends it
with llm 0.11.0's `llm-bedrock` client (SigV4-signed, Anthropic `InvokeModel` or Converse), and
encodes the answer back on the client's wire. Credentials come from the container credentials
endpoint EKS Pod Identity and ECS task roles serve (`AWS_CONTAINER_CREDENTIALS_FULL_URI`, with the
authorization token read from `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` through the gateway's
trusted-file reader). No AWS SDK crate, no aws-lc.

## Why

The owner wants the gateway to reach Anthropic and other models on Bedrock from a cluster whose
pods get an IAM role through Pod Identity, without a long-lived key in a file. llm 0.11.0 ships the
client; the gateway composes it as one more hosted target kind.

## The rule

- **Declaration.** `kind = "bedrock"` (default `endpoint`, today's hosted endpoint), `region` (an
  AWS region name), `model_id` (a model or inference-profile id), `credential_source = "container"`
  (the only value), optional `api` (`anthropic-invoke` | `converse`; default `anthropic-invoke` for
  an id naming `anthropic.`, `converse` otherwise), optional `endpoint` (an https origin, or http
  to a loopback literal, for a VPC endpoint or a fixture), plus `context_window`, `tool_calling`
  and `billing_kind` as any hosted entry. `wires`, `base_url`, `auth_kind`, `credential_file`,
  `credential_header`, `headers`, `upstream_model` and `reasoning_effort` are refused on it, and
  the Bedrock keys are refused on an endpoint entry (`config:value`).
- **Always translated.** A Bedrock model is served on every wire and every request is translated;
  none is relayed byte for byte.
- **Dropped fields.** What the Bedrock client refuses or reports as an advisory omission for the
  turns Claude Code and typical chat clients send is dropped before the turn reaches the client,
  and the dropped field names (never prompt or answer text) are logged.
- **Credentials.** The two environment variables are read when the gateway starts; a Bedrock entry
  without them refuses the start. The token file is read through the trusted-file reader on every
  credentials fetch, so a rotated token is picked up.
- **Inventory.** One target per Bedrock model, provider `bedrock`, protocol the api's
  (`messages` or `bedrock-converse`); no region, model id, endpoint or credentials URI.
- **Dependencies.** Every llm dependency moves from 0.7.0 (`b10x-llm-*`) to 0.11.0 (`llm-*`).

## Acceptance

1. Loopback fixtures only (a fake Bedrock Runtime and a fake container credentials endpoint): a
   chat request with tools (streamed tool-call deltas and non-streamed), a `max_tokens: 1` warm
   probe, and Claude Code's recorded first turn on the messages wire are each answered on the
   client's wire, the fake Bedrock receiving a SigV4-signed request whose credentials came from
   the credentials endpoint with the token file's authorization.
2. The fields dropped for Claude Code's first turn are named in the log and absent from the body
   Bedrock receives; no log line carries prompt or answer text.
3. The config refusals above are `config:value`; a missing credentials variable refuses the start.
4. `ess specify validate --path spec` passes with the Bedrock kind in `HostedEndpoint` and
   `HostedDeclaration`; new scenarios are listed, the suite and schema regenerated, the baseline
   raised.
5. `cargo tree` and `Cargo.lock` hold no aws-lc and no AWS SDK crate.
6. `docs/gateway.md`, the site, README, AGENTS.md and CHANGELOG describe the kind; the example
   model id and the Pod Identity manifest are marked unverified.
