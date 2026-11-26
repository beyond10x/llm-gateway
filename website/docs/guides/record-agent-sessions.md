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
why. A refused call is a turn marked incomplete with the reason `refused`.

No message text is stored. A prompt, a tool input and a tool output are a digest and a byte
count. A session is named by the gateway's session hash, never by the header the client sent.

## The table

Add it to the deployment document:

```toml
[interaction_records]
path = "/srv/gateway-sessions/sessions.db"
```

| Key | Default | What it is |
| --- | --- | --- |
| `path` | required | The SQLite file. A relative path is resolved from the gateway's working directory. Its directory must exist when the gateway starts; the file is created there. |
| `queue` | `1024` (1 to 65536) | Finished calls waiting for the writer thread. |
| `call_bytes` | `8388608`, 8 MiB (1 to 33554432) | The most bytes of request body and answer body together that the gateway copies of one call. |
| `sessions_held` | `4096` (1 to 65536) | Sessions whose agents the gateway remembers between calls. |

Any other key refuses the document with `config:schema`, and a value outside its range with
`config:value` naming the key.

Before it writes, the gateway reads the file's schema version. The start is refused with
`config:value` naming `interaction_records.path`, never the path itself, when:

- the directory of `path` does not exist;
- the file is no session store (another program's SQLite file, or not SQLite at all);
- the file holds another schema version than the one this gateway writes. The message names
  both versions.

The file is then left as it was. A missing or empty file is created as a new store.

Without the table the gateway copies no byte of any call, runs no writer thread and creates no
file.

## A file of its own

Give the gateway a file of its own, in a directory of its own. Never point it at the
`sessions.db` sessionspec keeps in its own state directory for the transcripts it lifts.

The reason is the schema version. sessionspec empties a store of another schema version when it
opens it, and starts it again in its own version. A sessionspec of another version that opens
the gateway's file empties the gateway's records. The other way round the gateway refuses to
start, as above.

## What is held, and what is left out

The call is lifted on its own connection's thread, after its answer was written, and handed to
one writer thread through the queue; the client never waits for the file.

These calls make no record:

- A token count (`POST /v1/messages/count_tokens`) is no model turn: it makes no record and is
  not counted.
- A call that finds the queue full. It is counted in `llmgw_interaction_records_dropped_total`.
- A call whose request and answer together are over `call_bytes`, or whose answer carried a
  content coding the gateway could not decode. Its answer reaches the client unchanged. It is
  counted in `llmgw_interaction_records_skipped_total`.
- A call whose request the lift cannot read, for example a body that is not JSON, refused
  `body-not-json`: its lift yields no step. It is counted in
  `llmgw_interaction_records_skipped_total` too.
- A call the writer failed to store, for example because the file was removed or the disk is
  full. It is counted in `llmgw_interaction_records_failed_total`, and the gateway logs one
  warning per call naming that series, never the path.
- A call still queued 2 s after a stop began (below).

The counters are on `GET /metrics` ([Scrape the counters](scrape-the-counters.md)).

To tell one agent from another the gateway remembers, per session, each agent's conversation and
the spawn calls it made. It remembers at most `sessions_held` sessions; past that the session
used least recently is forgotten. The next call of a forgotten session continues under the
same agent id, so the store still shows one agent. What is lost is the chain state of its
earlier turns: which tool calls were still open, which spawn call started which agent, and which
response ids belong to which agent. A sub-agent that starts after its session was forgotten is
not linked to the call that spawned it. Raise `sessions_held` when more sessions than that are
active at once.

## Memory

- A held session keeps its lift state: per agent its id and open tool calls, one entry per
  model turn (its response id) and one per spawn call. It grows with each model turn and each
  spawn for as long as the session is held. `sessions_held` bounds how many sessions are held,
  not how large one grows.
- A call's byte copy, at most `call_bytes`, is held on the call's own thread until its lift
  ends; while it is lifted it is held twice.
- A queued call holds the steps lifted from it: digests, counts and ids, no text. Plan for
  `queue` times `call_bytes` as the queue's worst case (8 GiB at the defaults), and lower `queue`
  or `call_bytes` to lower it.

## At the stop

At SIGINT or SIGTERM the gateway first answers every connection it accepted, as without the
table. Then it waits at most 2 s for the writer to store what is queued. Calls still queued after
that are not stored, and the gateway names them on one line before its stop line:

```text
b10x-llm-gateway: interaction records: 3 calls not stored at stop
b10x-llm-gateway: stopped by SIGTERM accepted=12 completed=12
```

The stop line itself is unchanged, and so is the exit status 0.

## Read the file with sessionspec

`sessionspec serve` shows the sessions of the store in its state directory, `sessions.db` there.
Start a sessionspec that writes the same schema version (one built on `sessionspec-records`
0.1.0, schema version 4, for this gateway) on the gateway's own directory:

```bash
sessionspec --state-dir /srv/gateway-sessions serve
```

It serves its session list and session view on `http://127.0.0.1:7420`. A session the gateway
wrote is listed under its session hash.
