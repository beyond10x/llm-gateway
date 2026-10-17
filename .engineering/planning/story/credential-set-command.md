---
format: aep.planning-md/3
id: story:credential-set-command
kind: story
status: active
title: The owner stores an upstream key with b10x-llm-gateway credential set
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/llm-gateway-cli/Cargo.toml
- confidence: inferred
  path: crates/llm-gateway-cli/src/cli.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/credential.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/lib.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/refusal.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/routes.rs
- confidence: inferred
  path: crates/llm-gateway-cli/src/serve.rs
- confidence: inferred
  path: crates/llm-gateway-cli/tests/credential_set.rs
- confidence: inferred
  path: crates/llm-gateway-docs/src/status.rs
- confidence: inferred
  path: spec/domains/deployment.yaml
- confidence: inferred
  path: website/docs/guides/store-an-upstream-key.md
- confidence: inferred
  path: website/docs/reference/deployment-document.md
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:23:01Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-17T05:23:01Z", actor: "human:timo", revision: 4}
---
## Outcome

`b10x-llm-gateway credential set --config <gateway.toml> --model <alias>` (or `--route <name>`
for a route of the routes document the deployment names) stores one upstream key in the file the
deployment already names for it, so that the next start of the gateway reads it. It is the
official, documented way to store a key; until now an operator wrote the file by hand.

## Why

The owner asked for it: a hosted endpoint's key (OpenAI, OpenRouter, Anthropic, …) has to land in
an owner-only file the trusted-file reader accepts, and writing that file by hand gets the mode,
the trailing bytes or the ownership wrong, which the gateway only reports at its next start. A
second writer, connectors' custody delivery, will write the same kind of file; the reference states
the reader's rules so that both writers aim at one reader.

## The rule

- **Target.** Read from the document only: the hosted entry's `credential_file`, `auth_kind`,
  `credential_header`, `base_url` and static `headers`; for `--route`, that route's endpoint's
  `file:` secret path (a `~/` path below `HOME`), auth and base URL. No provider preset in code.
  Exactly one of `--model` and `--route`. Loading the target reads the document (and the routes
  document for `--route`) and no credential file, owner secret or key. Refused: an unknown alias or
  route, an anonymous endpoint, a model that is not a hosted endpoint.
- **Key input.** A hidden prompt on `/dev/tty` (echo off, restored on every path), or `--stdin` for
  scripts; never an argument or an environment variable. One line: the terminator (`\n`) ends the
  key and is not part of it. Refused: empty, any other CR or LF, leading or trailing whitespace,
  anything but one printable ASCII token of at most 4096 bytes (the reader's token rule).
- **Probe** (default; `--no-probe` skips it): `GET <base_url>/models` (the base URL with its path,
  so `https://api.openai.com/v1` probes `/v1/models`) with the entry's own credential header
  (`authorization: Bearer <key>` or the `api-key` header it names) and its static headers (so
  Anthropic's `anthropic-version` is sent), over the TLS client the gateway uses for hosted
  endpoints. No model call. 401 or 403 refuses and writes nothing; a transport failure refuses and
  writes nothing (`--no-probe` to store anyway); any other status stores and says which code.
- **Write.** A temporary file in the target's directory, created exclusively with mode 0600 and
  without following a symlink, written, flushed, then read back through the same trusted-file
  reader and token rule the server applies at start (`trusted::read` and `keys::read_token` with
  the endpoint-credential policy); only then renamed over the target. A refusal at any step leaves
  the target as it was and no temporary file behind. The target directory must exist. The file
  holds exactly the key's bytes.
- **Output.** One line on success,
  `b10x-llm-gateway: credential stored <path> probe <passed|skipped|not-rejected> http <code|->`,
  exit status 0; a refusal is `b10x-llm-gateway: refused <code>: <message>`, exit status 1; a
  command-line error is clap's, exit status 2. No line, event or refusal carries the key or any
  part of it.
- **Compatible.** `b10x-llm-gateway --config <file>` with no subcommand serves exactly as before.

## Acceptance

1. `ess specify validate --path spec` passes with the command, its target rule and its refusal
   codes modelled in `llm-gateway.deployment`; the regenerated `contracts/` match.
2. Tests (`crates/llm-gateway-cli/tests/credential_set.rs`) drive the binary with `--stdin` against
   loopback HTTP(S) fixtures only, and show: a 200 probe stores a 0600 file the gateway then starts
   with; 401 and 403 leave an existing target byte-for-byte unchanged and no temporary file; a
   transport failure refuses; `--no-probe` sends no request; the probe carries the entry's
   credential header and its static headers and asks for `<base path>/models`; every input rule
   above refuses; a `--route` target resolves its `file:` path; stdout and stderr never contain
   the key.
3. A write whose result the reader would refuse (for example a target directory the user does not
   own) is refused and leaves no file.
4. `b10x-llm-gateway --config <file>` with no subcommand passes every existing binary test.
5. The site has a guide page "Store an upstream key" and a reference entry that states the
   reader's rules for a credential file (regular file, not a symlink, owned by the user or root,
   no group or world permission bit, at most 4098 bytes, UTF-8, one printable ASCII token of at
   most 4096 bytes once trailing ASCII whitespace is trimmed); the generated CLI and refusal
   references list the command and its codes; README, `AGENTS.md`, `CHANGELOG.md` and the status
   page name it.
6. No test reaches a network address other than loopback.
