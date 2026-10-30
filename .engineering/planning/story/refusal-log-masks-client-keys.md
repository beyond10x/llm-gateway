---
format: aep.planning-md/3
id: story:refusal-log-masks-client-keys
kind: story
status: active
title: The translation refused line writes a client's key as ?
relations:
- serves: vision:portable-model-inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T07:21:28Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T07:21:28Z", actor: "human:timo", revision: 3}
---
## Outcome

The `translation refused` log line names where the refused field sits (`messages[].content[]`,
`text`, the root) and never a key the client chose. llm's diagnostics that copy a client's key
through `field_path` or `path` (a key of at most 64 bytes of `[A-Za-z0-9_-]`) are logged with
that key written `?`. Field names from llm's own fixed lists (`` field `n` ``, `text.format`,
`thinking.display`) stay named.

## Why

`review-result:adversary-w03-refusal-pass-1` finding 4: a client key of 64 bytes of
`[A-Za-z0-9_-]` shaped as a secret (`sk_live_…`) is copied into the info line. Wave w03 kept it
as llm's design; the goal of this session is every review finding fixed, and a key the client
chose is caller text whatever its shape, as wave w01 decided for `credential:unknown-target`
("no shape rule tells them apart").

## Acceptance

- For each ingress wire (chat, responses, messages) a request with a secret-shaped unknown key at
  the root and nested logs one `translation refused` line holding the location and `?`, and the
  key in no log line.
- A refusal naming a field from llm's fixed list still names it.
- `docs/gateway.md` states the rule.
