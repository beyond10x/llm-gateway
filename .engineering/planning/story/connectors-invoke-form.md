---
format: aep.planning-md/3
id: story:connectors-invoke-form
kind: story
status: implemented
title: The Runpod transport calls connectors 0.60.0's invoke form
relations:
- serves: vision:portable-model-inference
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T06:19:47Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T06:19:48Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-30T06:59:26Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

The Runpod transport over connectors calls the command form connectors 0.60.0 keeps:
`connectors --output json invoke <connector>/<instance> <operation> --connection <c>
--input-json <json>` (plus the pins and approval file it passes today), and reads the answer
connectors 0.60.0 prints, which names `connector` and `instance` and carries no `adapter`. No
source, test, fixture, specification comment or document names `operations invoke`,
`invoke --endpoint`, `describe --endpoint` or `serve`.

## Why

connectors 0.60.0 (CHANGELOG, `## 0.60.0`, Breaking) removes `operations invoke`,
`invoke --endpoint`, `describe --endpoint` and `serve`; each old command line now exits 2 and
contacts nothing. `crates/llm-runpod/src/transport/connectors.rs` builds every call as
`connectors --output json operations invoke --adapter <a> --connection <c> ...`, so against
0.60.0 every create, list, get, terminate and billing read is refused. The orchestrator placed
the move in this repository's next wave (`b10x-chef-orchestrator`, 2026-10-30, D263).

## Acceptance

- The fixture `crates/llm-runpod/tests/fixture/connectors.rs` accepts only the 0.60.0 command
  form and exits 2 on the removed one, as connectors 0.60.0 does; the transport's tests and the
  CLI tests that count calls (`calls_of`) pass against it.
- A test gives the transport an answer in 0.60.0's shape (`connector`, `instance`, no `adapter`)
  and the call succeeds.
- `grep -rn 'operations invoke'` over `crates/`, `spec/` and `docs/` finds nothing.
