---
title: Record agent sessions
sidebar_position: 12
description: Store every model call the gateway serves as session records in a SQLite file, the same records agentspec lifts from transcripts, and read them with agentspec serve.
lede: With an [interaction_records] table the gateway lifts each finished model call into agentspec's session records (sessions, agents, prompts, model turns, tool calls) and appends them to one SQLite file, without message text and without slowing an answer.
source: spec/domains/deployment.yaml (InteractionRecordsDeclaration), crates/llm-gateway-cli/src/records.rs, crates/llm-gateway-cli/tests/interaction_records.rs
---

# Record agent sessions

The gateway sees every model call an agent makes. With the table below it turns each finished
call into the records [agentspec](https://github.com/beyond10x/agentspec) lifts from a
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
- the file holds a later schema version than the one this gateway writes (5), or a schema row
  that does not read as agentspec writes a version (`4`, not `04`). The message names both;
- another process holds the file locked for longer than 2 s. The message says so: the file may
  be a valid store.

The file is then left as it was. A missing or empty file is created as a new store, and so is a
file that holds only agentspec's `meta` table without a schema row, which a first start
stopped during its setup leaves.

A store of an earlier schema version, such as the schema 4 an earlier gateway wrote, is migrated
when the gateway opens it at start: the tables agentspec derives are built again from the
store's wire records, which are kept, and the next calls are appended after them. A store whose
wire records are in a format this agentspec cannot read refuses the start, and nothing in it
changes.

Without the table the gateway copies no byte of any call, runs no writer thread and creates no
file.

## A file of its own

Give the gateway a file of its own, in a directory of its own. Never point it at the
`sessions.db` agentspec keeps in its own state directory for the transcripts it lifts.

The reason is the schema version. An agentspec of another schema version that opens the file
rebuilds the tables it derives in its own version and keeps the gateway's wire records. From
then on the file holds that version: an earlier one the gateway migrates again at its next
start, a later one it refuses, as above. Read the gateway's file with an agentspec of the same
schema version.

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
  full. It is counted in `llmgw_interaction_records_failed_total`, each call once. The gateway
  logs at most one warning per second, naming how many calls failed since the last warning and
  that series, never the path.
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

Within a held session the gateway keeps the 4,096 most recent response ids, the 4,096 most
recent conversation ids and the 4,096 most recent spawn inputs. A call that continues from an
older one, or a sub-agent started by an older spawn call, starts a new agent with no parent.

## Memory

- A held session keeps its lift state: per agent its id and open tool calls, and its links: at
  most 4,096 response ids, 4,096 conversation ids and 4,096 spawn inputs, the most recent kept.
  The links stop growing at that cap; the agents and their open tool calls grow with the
  session for as long as it is held. `sessions_held` bounds how many sessions are held.
- A call's byte copy, at most `call_bytes`, is held on the call's own thread until its lift
  ends; while it is lifted it is held twice.
- A queued call holds the steps lifted from it: digests, counts and ids, no text. Plan for
  `queue` times `call_bytes` as the queue's worst case (8 GiB at the defaults), and lower `queue`
  or `call_bytes` to lower it.

## At the stop

At SIGINT or SIGTERM the gateway first answers every connection it accepted, as without the
table. Then it waits at most 2 s for the writer to store what is queued. Calls still queued after
that are not stored. The gateway counts them, and the calls that failed since its last warning,
on one line before its stop line:

```text
b10x-llm-gateway: interaction records: 3 calls not stored at stop
b10x-llm-gateway: stopped by SIGTERM accepted=12 completed=12
```

The stop line itself is unchanged, and so is the exit status 0.

## Read the file with agentspec

`agentspec serve` shows the sessions of the store in its state directory, `sessions.db` there.
Start an agentspec that writes the same schema version (agentspec 0.2.0, schema version 5, for
this gateway) on the gateway's own directory:

```bash
agentspec --state-dir /srv/gateway-sessions serve
```

It serves its session list and session view on `http://127.0.0.1:7420`. A session the gateway
wrote is listed under its session hash.
