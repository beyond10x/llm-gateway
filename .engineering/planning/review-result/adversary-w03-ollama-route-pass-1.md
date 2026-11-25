---
format: aep.planning-md/3
id: review-result:adversary-w03-ollama-route-pass-1
kind: review-result
status: active
title: Adversary, pass 1 (story:ollama-route)
relations:
- reviews: story:ollama-route
revision: 1
---
unit: story:ollama-route
verdict: red
cases: executed 562→565, red 2
origin: introduced 2, pre-existing 0, undecided 0
wrote-outside-worktree: <scratch>/run-1-cases-alone.log, <scratch>/run-mutant-m1.log, <scratch>/run-2-suite.log, <scratch>/run-3-deselected.log, <scratch>/wake_status.diff, <scratch>/wake_status.rs.orig, <scratch>/review-pass-1.md
needs-coordinator: no

These findings cover branch impl/ollama-route at 8895dd4e plus my uncommitted test cases.

## 1. Diff stat

```
 crates/llm-gateway-cli/tests/wake_status.rs | 166 ++++++++++++++++++++++++++++
 1 file changed, 166 insertions(+)
```

Only a test file changed, and only by additions: a new module `adversary_ollama_route` at the end of the file. No existing case was edited. Mutant M1 was applied to `crates/llm-gateway-cli/src/hosted.rs:569` and reverted. `git status` now shows only the test file. The brief allowed the mutant in the worktree itself; the adversary procedure says to mutate a copy. I followed the brief.

## 2. Cases added (`crates/llm-gateway-cli/tests/wake_status.rs`, module `adversary_ollama_route`)

All three go through FakeOllama or a raw server on loopback, port 0. Run alone, the three finished in 0.09 s together, so none is over 5 s. Command: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked -p b10x-llm-gateway-cli --test wake_status adversary_ollama_route` (log `<scratch>/run-1-cases-alone.log`, exit 101, run on build-1d).

### a. `adv_ollama_a_prefixed_name_the_server_lists_is_relayed_not_refused`: RED now

- **Setup:** loads the document through `load` and serves it through `start_relaying`. The model is `[hosted.local] kind = "ollama"`, `upstream_model = "qwen2.5:1.5b"`, `model_prefix = "llama"`. The server has pulled only `llama3.2:latest`.
- **Call:** a chat call names `llama3.2:latest`.
- **Asserts:** a 200 answer.
- **Holds:**
  - Acceptance 4: "A call to a model that is not pulled is refused". Here the model the call is sent as is pulled.
  - The brief's first concern: "a status or a refusal that says something other than what the server answered".
  - The deployment reference on `model_prefix`: "every model name starting with it is served by this model too, relayed as the client sent it". The Ollama section of that reference says "every other endpoint key keeps its rule above".

Red output (run alone, before the suite):
```
thread 'adversary_ollama_route::adv_ollama_a_prefixed_name_the_server_lists_is_relayed_not_refused' (3521865) panicked at crates/llm-gateway-cli/tests/wake_status.rs:1958:9:
the server lists llama3.2:latest, the call names it, and the gateway answered: HTTP/1.1 503 Service Unavailable
content-type: application/json
content-length: 133
cache-control: no-store
connection: close

{"error":{"code":"model-not-pulled","message":"the model is not pulled on its Ollama server, and the gateway pulls no model: local"}}
request lines the server saw: ["GET /api/tags"]
```

### b. `adv_ollama_a_prefixed_name_the_server_does_not_list_is_refused_model_not_pulled`: RED now

- **Setup:** the same table. The server has pulled only `qwen2.5:1.5b`.
- **Call:** a chat call names `llama3.2`.
- **Asserts:** 503 `model-not-pulled`, and no `POST` reaches the server.
- **Holds:** Acceptance 4, "A call to a model that is not pulled is refused `503` with a code of its own, `model-not-pulled`". Also the decided point that the refusal is decided before the call is relayed.

Red output:
```
thread 'adversary_ollama_route::adv_ollama_a_prefixed_name_the_server_does_not_list_is_refused_model_not_pulled' (3521863) panicked at crates/llm-gateway-cli/tests/wake_status.rs:1980:9:
the server does not list llama3.2 and the gateway answered: HTTP/1.1 404 Not Found
content-type: application/json; charset=utf-8
cache-control: no-store
connection: close
transfer-encoding: chunked

64
{"error":{"message":"model 'llama3.2' not found","type":"not_found_error","param":null,"code":null}}
0


wire calls the server received: ["POST /v1/chat/completions"]
```

### c. `adv_ollama_the_list_bound_flips_at_exactly_the_published_number_in_every_framing`: green now; kills mutant M1

- **Setup:** a `GET /api/tags` body that lists the model, padded to 1048576 and to 1048577 bytes. Each size is sent framed by `content-length`, chunked, and read to the close.
- **Asserts:** 1048576 bytes give `stopped` `not-loaded`; 1048577 bytes give `unreachable` `no-answer`.
- **Holds:** `docs/gateway.md` "Bounds of the status probe" (`ollama-list-max-bytes`, "framed by `content-length`, chunked or read to the end of the connection … a longer one is `no-answer`"). Also the AGENTS.md invariant "every bound flips at exactly the published number".
- **Why it was missing:** the implementor's only check of this bound is the constant pin in `route_status_the_published_probe_bounds_are_the_defaults` (in `hosted.rs` tests).

## 3. Suite run (after the cases existed)

Command: `RUSTUP_TOOLCHAIN=1.98.0 SLOT_TIMEOUT=30m cargo test --locked -p b10x-llm-gateway-cli --no-fail-fast`. Log `<scratch>/run-2-suite.log`, exit 101, run on build-1d.

```
test result: FAILED. 20 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.10s
error: test failed, to rerun pass `-p b10x-llm-gateway-cli --test wake_status`
error: 1 target failed:
    `-p b10x-llm-gateway-cli --test wake_status`
```

- **Totals over all targets of the crate:** 563 passed, 2 failed (my cases a and b), 1 ignored. That is 565 executed.
- **The before number** comes from a second run with my module deselected: `cargo test --locked -p b10x-llm-gateway-cli --test wake_status -- --skip adversary_ollama_route`. It gave `test result: ok. 19 passed; 0 failed; ... 3 filtered out`, exit 0 (`<scratch>/run-3-deselected.log`). So before is 565 − 3 = 562.
- **Slow binaries (not mine):** two took 12.01 s and 10.19 s.

## 4. Findings

### F1: with a `model_prefix`, an Ollama model's status and `model-not-pulled` check one model while the call sends another

**NEEDS-CHANGE, introduced, blocker.**

What was measured:
- Case a fails at `wake_status.rs:1958`: the server lists the model the call names, and the call is refused 503 `model-not-pulled`.
- Case b fails at `wake_status.rs:1980`: the server does not list the model the call names, and the call is relayed into Ollama's own 404.

Why:
- `HostedTargets::acquire` (`crates/llm-gateway-cli/src/hosted.rs:1029-1033`) decides from the status of `HostedEndpoint::ollama`, which is always `upstream_model`.
- `rewrite` (`crates/llm-gateway/src/relay.rs:2509-2512`) sends the client's own name when a prefix matched.

What reaches it:
- `validate_ollama` (`crates/llm-gateway-cli/src/config.rs:1508`) calls `validate_hosted`, which admits `model_prefix` (`config.rs:1436`).
- The deployment reference's Ollama section keeps every other endpoint key, and the cases load such a document through `load`.
- `docs/gateway.md` names the `model_prefix` case only for the alias in the message, so the implementor saw this configuration.

Base: 1a6234fc had no `kind = "ollama"` (`from_string_only!(HostedKindDocument { "endpoint" => Endpoint, "bedrock" => Bedrock })`), so the defect is introduced.

Possible fix, not applied: refuse `model_prefix` on `kind = "ollama"` as `config:value` naming the key. The other way is to read tags and ps for the name the call is sent as.

### F2: the list bound `ollama-list-max-bytes` had no behavioural test; mutant M1 survived the implementor's tests

**CONFIRMED, introduced, warning.**

Mutant M1, `crates/llm-gateway-cli/src/hosted.rs:569`:

```
-        if body.len() > OLLAMA_LIST_MAX_BYTES {
+        if wanted.is_some() && body.len() > OLLAMA_LIST_MAX_BYTES {
```

This removes the bound on a body read to the close of the connection. I ran `cargo test --locked -p b10x-llm-gateway-cli --test wake_status` against it (`<scratch>/run-mutant-m1.log`):
- Every `ollama_*` case of the implementor passed.
- Only my case c failed: `assertion 'left == right' failed: to-the-close, a tags body of 1048577 bytes; left: Some(ModelStatus { phase: Stopped, sub_phase: Some("not-loaded"), profile: None }); right: Some(ModelStatus { phase: Unreachable, sub_phase: Some("no-answer"), profile: None })`.

The mutant was reverted right after the run. The lib unit tests were not run under it; the only lib test that names the constant is the constant pin.

Case c now closes this gap. The row stays because the implementor's suite had the gap.

### Judgement findings with no case

None.

## 5. Attacked and could not break

- **Request lines:** over a status read, a wake, the list, a call on each wire and a refused call, only `GET /api/tags`, `GET /api/ps` and the wire `POST`s are sent. I read the code: wake returns `None`, and `credential set` refuses an anonymous model, so no `GET /v1/models` is sent. The implementor's binary-composition test checks the same set.
- **Status mapping:** a non-2xx status is `status-<code>`; a redirect is not followed (`final_status` returns the 3xx). A 2xx that is not a list, a silent server and a slow ps after a fast tags are all `no-answer` inside one deadline. Read, plus the implementor's tests.
- **Tag rule:** only the last `/` segment decides whether a name has a tag. Matching is exact string equality, so no two different models are made equal. A namespaced name or a host with a port is handled.
- **Refusal body:** only `model-not-pulled` carries the alias (`refusal_body` filter, `server.rs:1906`). The alias is the declaring alias (`relay.rs:1975`), written through the JSON writer. Every other refusal is passed `None`, or its code is filtered out.
- **Loader:** a credential key, another `auth_kind`, a Bedrock key and a `base_url` with a path, userinfo or query are refused naming the key. A document without an Ollama model keeps `billing_kind` as before (`unwrap_or_default`). Checked by reading the code and the implementor's config tests; I added no case.
- **The four mutants the brief names: killed by reading, not run.**
  - Skip `/api/ps`: caught by the request-line and phase assertions of `ollama_status_is_read_from_tags_then_ps_and_sends_nothing_else`.
  - Make `acquire` refuse when the server did not answer: caught by the "no answer is no refusal" assertion of `ollama_acquire_refuses_only_a_model_the_server_answered_without`.
  - Let a status read send a wire call: caught by the request-line equality in the status tests.
  - Drop the tag rule: caught by the case `qwen2.5` against `qwen2.5:latest`, which expects `ready`.
- **Not attacked:** the usage record of a translated wire and of streamed calls, and whether Ollama 0.40.18 serves `POST /v1/messages/count_tokens` (the facts in the brief do not say).

## 6. Paths written outside the worktree

- `<scratch>/run-1-cases-alone.log`
- `<scratch>/run-mutant-m1.log`
- `<scratch>/run-2-suite.log`
- `<scratch>/run-3-deselected.log`
- `<scratch>/wake_status.diff` (the implementor's test diff, kept for reading)
- `<scratch>/wake_status.rs.orig` (a copy of the test file before my additions)
- `<scratch>/review-pass-1.md` (this report)

`<scratch>` is the pass's scratch directory. The build and the fixture directories (`CARGO_TARGET_TMPDIR`, `gateway-cli-adv-ollama-*`) are under the worktree's `target/` and the build node's mirror of it.

## 7. Findings block

```findings
- file: crates/llm-gateway-cli/src/hosted.rs
  line: 1029
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: with a model_prefix on a kind = "ollama" table, acquire refuses or admits a call from the status of upstream_model while the call is sent under the client's own name, so a model the server lists is refused model-not-pulled and a model it does not list is relayed into Ollama's 404
- file: crates/llm-gateway-cli/src/hosted.rs
  line: 569
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: no implementor test exercised ollama-list-max-bytes; removing the bound on a list body read to the close passed every implementor wake_status case and is caught only by the added case adv_ollama_the_list_bound_flips_at_exactly_the_published_number_in_every_framing
```
