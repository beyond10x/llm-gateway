---
title: Record agent sessions
sidebar_position: 12
description: Store every model call the gateway serves as session records in a SQLite file, the same records sessionspec lifts from transcripts, and read them with sessionspec serve.
lede: With an [interaction_records] table the gateway lifts each finished model call into sessionspec's session records (sessions, agents, prompts, model turns, tool calls) and appends them to one SQLite file, without message text and without slowing an answer.
source: spec/domains/deployment.yaml (InteractionRecordsDeclaration), crates/llm-gateway-cli/src/records.rs, crates/llm-gateway-cli/tests/interaction_records.rs
---

# Record agent sessions

The gateway sees every model call an agent makes. With the table below it turns each finished
call into the records [sessionspec](https://github.com/beyond10x/sessionspec) lifts from a
harness's transcripts: the session, its agents and which agent spawned which, each prompt, each
model turn with its usage, and each tool call. Every model turn also carries what only the
gateway knows: the call id (`x-llm-gateway-call`), the route, the upstream, the attempts, the
queue wait, the time to first token, the declared-price cost and, when the answer was not whole,
why. A refused call is a turn marked incomplete with the reason `refused`. A token count
(`POST /v1/messages/count_tokens`) is no model turn and makes no record.

No message text is stored. A prompt, a tool input and a tool output are a digest and a byte
count. A session is named by the gateway's session hash, never by the header the client sent.

## The table

Add it to the deployment document:

```toml
[interaction_records]
path = "/srv/sessions/sessions.db"
```

| Key | Default | What it is |
| --- | --- | --- |
| `path` | required | The SQLite file. A relative path is resolved from the gateway's working directory. Its directory must exist when the gateway starts; the file is created there. |
| `queue` | `1024` (1 to 65536) | Finished calls waiting for the writer thread. |
| `call_bytes` | `8388608`, 8 MiB (1 to 33554432) | The most bytes of request body and answer body together that the gateway copies of one call. |
| `sessions_held` | `4096` (1 to 65536) | Sessions whose agents the gateway remembers between calls. |

Any other key refuses the document with `config:schema`, and a value outside its range with
`config:value` naming the key. A `path` whose directory does not exist, or a file that does not
open as a session store, refuses the start with `config:value` naming `interaction_records.path`.

Without the table the gateway copies no byte of any call, runs no writer thread and creates no
file.

## What is held, and what is left out

The call is lifted on its own connection's thread, after its answer was written, and handed to
one writer thread through the queue; the client never waits for the file.

- A call that finds the queue full is not recorded. It is counted in
  `llmgw_interaction_records_dropped_total`.
- A call whose request and answer together are over `call_bytes`, or whose answer carried a
  content coding the gateway could not decode, is not recorded either. Its answer reaches the
  client unchanged. It is counted in `llmgw_interaction_records_skipped_total`.

Both counters are on `GET /metrics` ([Scrape the counters](scrape-the-counters.md)).

To tell one agent from another the gateway remembers, per session, each agent's conversation and
the spawn calls it made. It remembers at most `sessions_held` sessions; past that the session
used least recently is forgotten. The next call of a forgotten session opens it again in the
file and starts a new agent there, so one long-idle session can show the same agent twice. Raise
`sessions_held` when more sessions than that are active at once.

## Read the file with sessionspec

`sessionspec serve` shows the sessions of the store in its state directory, `sessions.db` there.
Name the gateway's file in that directory, and start sessionspec on the same directory:

```bash
sessionspec --state-dir /srv/sessions serve
```

It serves its session list and session view on `http://127.0.0.1:7420`. A session the gateway
wrote is listed under its session hash.
