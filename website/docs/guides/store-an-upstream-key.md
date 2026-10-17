---
title: Store an upstream key
sidebar_position: 6
description: Store a hosted endpoint's or a route's API key in the file the deployment document names, with b10x-llm-gateway credential set, checked against the endpoint first and written so the gateway's own reader accepts it.
lede: b10x-llm-gateway credential set reads the key without echoing it, asks the endpoint whether it accepts the key, and writes the file the document already names, mode 0600, so that the next start reads it.
source: crates/llm-gateway-cli/src/credential.rs, crates/llm-gateway-cli/tests/credential_set.rs, spec/domains/deployment.yaml (CredentialSetCommand); every output below is pasted from a run of the binary
---

# Store an upstream key

A hosted model (`[hosted.<alias>]`) or a route of a routes document reads its API key from a file
the deployment document names. The gateway reads that file once, at start, and refuses it if the
mode, the owner or the bytes are wrong. `credential set` writes it for you:

```text
b10x-llm-gateway credential set --config <file> --model <alias>
b10x-llm-gateway credential set --config <file> --route <name>
```

It takes nothing else from the document: no file it names is read, and no provider is built in.
The key is read from a hidden prompt on the terminal, or from standard input with `--stdin`; it
is never an argument or an environment variable, so it does not land in your shell history or in
the process list.

:::caution[Not run against a vendor API]
The command is tested against loopback endpoints only. The runs below use a local
OpenAI-compatible server; a vendor's `/models` path is assumed to answer as theirs is documented.
:::

## The entry names the file

The key goes where the entry says, under the entry's own authentication:

```toml title="gateway.toml"
listen = "127.0.0.1:8080"
owner_secret_file = "owner-secret"

[hosted.local]
wires = ["chat"]
base_url = "http://127.0.0.1:8000/v1"
auth_kind = "bearer"
credential_file = "keys/local"
```

A relative path is read from the directory the gateway starts in. For a vendor the entry differs
only in its URL, authentication and static headers:

```toml
[hosted.openai]
wires = ["chat", "responses"]
base_url = "https://api.openai.com/v1"
auth_kind = "bearer"
credential_file = "keys/openai"

[hosted.claude]
wires = ["messages"]
base_url = "https://api.anthropic.com/v1"
auth_kind = "api-key"
credential_header = "x-api-key"
credential_file = "keys/claude"

[hosted.claude.headers]
anthropic-version = "2023-06-01"
```

For `--route`, the key goes to the route's endpoint's `file:` secret, a `~/` path resolved
against `HOME`.

## Store it

The directory must exist, and be yours (or root's) and not group- or world-writable:

```shell
mkdir -m 700 keys
b10x-llm-gateway credential set --config gateway.toml --model local
```

```text
b10x-llm-gateway: key for hosted.local.credential_file (input hidden):
b10x-llm-gateway: credential stored keys/local probe passed http 200
```

Nothing is echoed while you type or paste; Enter ends the key; Ctrl-C, Ctrl-D or a hang-up cancels, and a
partial line is never stored. Before writing,
the command sends one `GET <base path>/models` with the key, as a request to the endpoint would
carry it: `authorization: Bearer <key>`, or the `credential_header` an `api-key` entry names, and
the entry's static headers. `https://api.openai.com/v1` is asked `/v1/models`, and a base URL
without a path is asked `/v1/models` too, the path the gateway itself relays a request for the
model list to. Interim `1xx` answers are skipped: the status is the first final one. The whole
probe, connecting, the TLS handshake and the answer, ends within 30 seconds. No model is called.

| Probe answer | What happens |
| --- | --- |
| `2xx` | stored, `probe passed` |
| `401` or `403` | refused `credential:probe-rejected`; nothing is written |
| no connection, an untrusted certificate, no answer within 30 seconds | refused `credential:probe-unreachable`; nothing is written |
| any other status | stored, `probe not-rejected`, with the status, since some endpoints have no `/models` |

The file holds exactly the key, with no newline, mode `0600` (the key above was 27 characters):

```shell
stat -c '%a %s' keys/local
```

```text
600 27
```

The key is written to a temporary file beside the target, read back through the gateway's own
reader and only then renamed over the old file, so a refused run leaves the old key in place and
no temporary file behind. Restart the gateway to use the new key.

## From a script

`--stdin` reads one line from standard input; the newline is not part of the key. `--no-probe`
stores without asking the endpoint, for an endpoint you cannot reach from here:

```shell
printf '%s\n' "$KEY" | b10x-llm-gateway credential set --config gateway.toml --model local --stdin --no-probe
```

```text
b10x-llm-gateway: credential stored keys/local probe skipped http -
```

A line that is not one printable ASCII token of at most 4096 bytes is refused before anything is
sent or written, and the message never quotes it:

```shell
printf ' %s\n' "$KEY" | b10x-llm-gateway credential set --config gateway.toml --model local --stdin
```

```text
b10x-llm-gateway: refused credential:input-not-a-token: the key must be one line holding one printable ASCII token: no space, tab, CR, second LF, control or non-ASCII byte, leading or trailing whitespace included
```

[Refusals](../reference/refusals.md#credential-refusals) lists every code, and the deployment
document reference states [the rules a credential file is read by](../reference/deployment-document.md#credential-files),
for anything else that writes one.
