# The authenticated single-owner gateway

`llm-gateway` admits exactly one authenticated owner and lets that owner read the routes a
deployment serves. This milestone is authentication, liveness, readiness, graceful shutdown and
read-only route inspection, and the relay of three model-call wires to a target the embedding
hands out ([below](#the-relay)). Protocol translation between wires, multi-tenant accounts and
quotas are not in it, and the crate contains nothing that could do them.

## What the crate cannot do, structurally

Nothing in the crate's build resolves a credential, opens a connection to an upstream or creates
a billed resource — the relay speaks only over connections its injected targets open — not `llm-credentials`, `llm-http`, `llm-provision`, `llm-runpod`, `llm-modal`,
`reqwest`, `hyper`, `axum`, `keyring-core` or `tokio`. `crates/llm-gateway/tests/dependency_boundary.rs`
asserts that over the **transitive closure** cargo itself resolves (`cargo tree`), not only over
the crate's declared list, and each of its two mechanisms carries a positive control that fails
if the mechanism stops seeing what it is supposed to see. Today the closure is one package: the
gateway declares nothing at all.

That makes "listing or explaining routes must not resolve secrets or provision resources" a
property of the dependency graph rather than a discipline — but only while the closure stays
clean. Taking the `llm-routing` dependency named below would pull `b10x-llm-credentials` and
`tokio` into it two edges away, which costs exactly this guarantee. That is a decision for
whoever takes it, and the boundary case is where it has to be made explicitly.

The gateway is never given a credential *source*. The embedding resolves the owner's material
once, from whatever source it chose, and hands over an `OwnerToken`. A server deployment
therefore never assumes a desktop keychain: see [explicit local secret sources](local-secrets.md)
for the resolvers an embedding may compose, all of which stay outside this crate.

## Composition

```rust
use llm_gateway::{Gateway, GatewayConfig, OwnerToken, SharedSecretVerifier};
use std::sync::Arc;

// `material` came from the embedding's own resolver, exactly once.
let token = OwnerToken::new(material)?;
let verifier = Arc::new(SharedSecretVerifier::new(token)?);
let handle = Gateway::bind(GatewayConfig::new("127.0.0.1:8080".parse()?), verifier, inventory)?;
handle.mark_ready();
```

`OwnerToken` is redacted in `Debug`, has no `Display`, no `Clone` and no serialisation, and its
bytes are overwritten on drop. `GatewayConfig` holds no credential at all, so a configuration
value can be logged or written to disk. `SharedSecretVerifier` compares the SHA-256 digests of the
expected and presented values in constant time, so the comparison depends on neither the content
nor the length of the expected secret. It refuses a shared secret below 32 bytes at composition
rather than serving a guessable one. Hashing the presented value takes time that depends only on
its own length, which its sender already knows.

An embedding that authenticates differently — a hardware token, a signed assertion, a reverse
proxy's verified header — implements `OwnerVerifier` instead. An implementation must redact its
own `Debug`.

## The route inventory

`RouteInventory` is an immutable snapshot of credential-free facts, built by whatever composed the
deployment. The natural source is `llm-routing`: `Catalog::routes` for the declarations and the
provenance that `Catalog::explain` already computes without resolving anything. The adapter from a
`Catalog` to a `RouteInventory` belongs to the composing binary, because this crate does not
depend on `llm-routing`; see the dependency boundary above.

A target summary names the protocol, provider, account, endpoint, model and binding revision by
their operator-defined identifiers, and the target's authentication and billing *kinds*. There is
no field for an endpoint URL, a secret reference name or credential material.

What that guarantees is narrower than it sounds, and the narrow version is the true one: an
inspection response contains **exactly the identifier bytes the composer supplied, and nothing
else**. `Label` accepts any 1..=256 printable US-ASCII bytes, which is what a catalog identifier
is — and also what a URL or an API key is. A composer that put a base URL into the `endpoint`
identifier would see that URL rendered. The crate cannot tell the difference, so it does not
claim to: `provenance_renders_exactly_the_bytes_the_composer_supplied` in `src/inventory.rs`
pins that behaviour rather than leaving it to a fixture that happens to choose opaque names.
**The adapter that builds a `RouteInventory` is responsible for passing identifiers.** The
`llm-routing` provenance this is modelled on carries exactly those identifiers, and carries the
base URL and the secret reference somewhere else.

**An unreported limit stays unreported.** `TargetLimits::context_window` and
`max_output_tokens` are `Option<u64>`; a limit the snapshot's source did not report is absent
from the rendered object. It never becomes `0` and never becomes a substituted configured value.
The same rule governs `config_digest`: a snapshot whose source reported no digest renders no
digest, rather than an empty string.

A snapshot that breaks the catalog's structural rules is not composed: a route with no target
(`inventory:route-without-target`) or more than `targets-per-route` targets, target positions
that do not run 0, 1, 2, … in order (`inventory:non-contiguous-positions`), one target
identifier twice on a route (`inventory:duplicate-target`), two routes with one alias
(`inventory:duplicate-alias`), more than `routes` routes, and a `config_digest` that is not
exactly `digest-characters` lowercase hexadecimal characters (`inventory:malformed-digest`).

## The HTTP surface

Every response is `cache-control: no-store` and `connection: close`, and every response the
gateway writes itself is `application/json` except the counters at `/metrics`, which are
Prometheus text, and the text and HTML answers of `GET /` ([the public routes](#the-public-routes));
a relayed answer keeps its target's content type.
The gateway serves one request per connection; keep-alive is not in this milestone.

| Method | Path | Credential | Meaning |
| --- | --- | --- | --- |
| `GET`, `HEAD` | `/health` | none | Liveness. Always `200 {"status":"live"}` while the process serves. |
| `GET`, `HEAD` | `/ready` | none | Readiness. `200 {"status":"ready"}`, or `503 {"status":"unready"}` before `mark_ready` and while draining. |
| `GET`, `HEAD` | `/v1/models` | none | The relayed models in the OpenAI list shape ([the public routes](#the-public-routes)). |
| `GET`, `HEAD` | `/` | none | Setup instructions chosen by `User-Agent`. |
| `GET`, `HEAD` | `/v1/routes` | owner | Every route in the snapshot. |
| `GET`, `HEAD` | `/v1/routes/{alias}` | owner | One route by alias. |
| `GET`, `HEAD` | `/metrics` | owner | The counters, as Prometheus text ([counters and usage records](#counters-and-usage-records)). |
| `POST` | `/v1/chat/completions` | owner, or none under `IngressAuth::None` | The `chat` wire, relayed ([the relay](#the-relay)). |
| `POST` | `/v1/responses` | owner, or none under `IngressAuth::None` | The `responses` wire, relayed. |
| `POST` | `/v1/messages` | owner, or none under `IngressAuth::None` | The `messages` wire, relayed. |

The unauthenticated surface is a closed set of four literal paths with two literal methods. The
two probes are matched before anything else — including before load is shed — so that a
liveness probe never carries the owner credential and never fails because the gateway is busy.
A liveness probe that fails under load restarts the process, which would turn shedding into a
restart loop. Reading a bounded request head under a timeout is the cheapest way to know whether
a request is a probe, so that happens first and nothing beyond the head is read before the
request is authenticated.

Load is shed immediately after the probe match and **before** authentication, as `overloaded`:
shedding has to be cheaper than the work it sheds. Shedding is not decoding. The two public
routes, `GET /v1/models` and `GET /`, are matched after shedding and before authentication; any
other method on them is authenticated like any other request.
**Every other request is authenticated before any further decoding**: an unauthenticated `POST`
to an unknown path is refused as `credential-absent`, not as `method-not-allowed` or
`path-unknown`, so the surface cannot be enumerated without the credential. A query string is
discarded; this milestone decodes no request parameter. Only a relay reads a request body, and
only after the owner is authenticated and the head has admitted it.

A head that is not HTTP/1.x, a head that is not UTF-8 (obs-text included), a header whose name
is followed by whitespace before its colon (RFC 9112 section 5.1), a CR, LF or NUL inside a line
(RFC 9110 section 5.5), and a head that has not arrived whole when `read_timeout` passes are each
`request-malformed`. The owner presents
`authorization: Bearer <token>`, the scheme matched without regard to case. Every `401` carries
`www-authenticate: Bearer`, and every `405` an `allow` header naming the methods the path takes.

## The public routes

`GET /v1/models` (row R5) and `GET /` (row R1) answer without a credential, so a client can be
told how to reach the gateway before it holds anything. They read the relayed models and nothing
else: never the route inventory, never the target source, never a credential. So neither wakes a
pod, and neither renders a provenance label, the `config_digest`, a target's endpoint, an upstream
model name, a bearer or any byte of the owner secret. A gateway composed without a relay lists no
model. Like inspection, a declared body is `body-not-allowed` and before `mark_ready` they are
`unavailable`. Another method is authenticated like any other request: `credential-absent`
without the credential, `method-not-allowed` with `allow: GET, HEAD` with it. `GET /v1/routes`
still takes the owner credential.

`GET /v1/models` is `{"object":"list","data":[…]}`, one entry per model in alias order:
`{"id":<alias>,"object":"model","owned_by":"llm-gateway","max_model_len"?:<n>,"wires":[…]}`.
`max_model_len` is what `RelayModel::with_max_model_len` declared, and absent, never `0`, when
nothing was.

`GET /` chooses its answer from the lowercased `user-agent`, by the first rule that holds:
containing `codex`, a Codex `config.toml` profile per model (`text/plain; charset=utf-8`);
containing `claude` or `anthropic`, Claude Code's environment variables per model (the same
type); starting with `mozilla/`, an HTML page (`text/html; charset=utf-8`); anything else, or no
`user-agent`, plain text with the llm catalog lines a Loom run needs. A client gets a profile for
a model only when the model declares the client's wire (Codex `responses`, Claude Code `messages`,
Loom `chat`) and parses tool calls; each profile names the model's alias and the
`context_window` from `RelayModel::with_context_window`. The base URLs use `http://` and the
request's `host` when it is only letters, digits, `.`, `-`, `_`, `:`, `[` and `]`, and the
listener's address otherwise. The owner token is named as the setting that carries it, never
given. Each `GET /` or `HEAD /` answered `200` counts one `llmgw_instruction_views_total`.

## The relay

`Gateway::bind_with_relay` composes the gateway with a `Relay`: the relayed models and an injected
`RelayTargets`. A `RelayModel` is an alias, the model name its target expects, a non-empty set
of distinct `Wire`s (`relay:no-wire`, `relay:repeated-wire`; two models with one alias are
`relay:duplicate-model`), its `ToolCalling`, `Parsed` or `Absent`, which is `Absent` unless
`with_tool_calling` says otherwise, an optional `ReasoningEffort` (`with_reasoning_effort`,
step 6), and the optional `max_model_len` and `context_window` the public routes publish, the
first of which declared is its window (`RelayModel::window`, step 7). The embedding implements `RelayTargets` over its pool: `acquire` hands
out the `RelayTarget` serving an alias, and `invalidate` is told when a request through one
failed. A `RelayTarget` opens its own connection (`connect`), so the transport, TLS included,
and its timeouts are the embedding's; this crate opens none. The specification is the `Relay`
command of `spec/domains/gateway.yaml`, and `crates/llm-gateway/tests/wire_relay.rs` names each
case after the row of the [capability matrix](llmgw-capability-matrix.md) it closes. A gateway
bound with `Gateway::bind`, without a relay, answers the three wire paths as it answers any
path it does not serve: after authentication, `path-unknown` to `GET` and `HEAD`, and
`method-not-allowed` with `allow: GET, HEAD` to any other method.

A request to a wire path is decided in this order:

1. Probes, shedding and owner authentication, exactly as for inspection. llmgw serves its wires
   without authentication; llm-gateway admits only its owner's bearer credential, unless the
   embedding sets `GatewayConfig::ingress_auth` to `IngressAuth::None`. Then a wire path of a
   gateway composed with a relay skips authentication: `authorization` is not read, so a
   missing, malformed or wrong credential is no refusal, and it is never forwarded. Every other
   path still authenticates, so `/metrics`, `/v1/routes` and an unknown path stay closed and the
   surface still cannot be enumerated. An embedding with no owner at all composes the gateway
   with `NoOwnerVerifier`, which accepts no credential: the owner-only paths are then
   `credential-absent` without one and `credential-rejected` with one. The binary does exactly
   this for `ingress_auth = "none"` without an `owner_secret_file`, and says so on standard
   error before it listens.
2. `POST` only (`method-not-allowed`, `allow: POST`), and `unavailable` before `mark_ready`.
3. The body: `content-length` or `transfer-encoding: chunked`, nothing else
   (`request-malformed`). A declared length over `request-body-bytes` is `body-too-large` before a
   byte is read; a chunked body is refused the moment its decoded size would pass the bound. A
   chunk size is `1*HEXDIG` and an optional extension after `;`, as RFC 9112 section 7.1 has it;
   anything else is `request-malformed`. The trailer section of a chunked body counts against
   `request-head-bytes` and is `request-too-large` past it. The whole body is read under one
   `read_timeout` deadline, and a body that ends or stalls before it is complete is
   `body-incomplete`. `expect: 100-continue` is answered for HTTP/1.1. A head holding a CR, LF
   or NUL anywhere but its line ends is `request-malformed` (RFC 9110 section 5.5).
4. One well-formed UTF-8 JSON object (`body-not-json`), with exactly one top-level `model`
   string (`model-absent`; a second top-level `model` is `body-not-json`, because the target
   could read the other), naming a relayed model after unescaping (`model-unknown`).
5. The model must declare the path's wire (`wire-not-served`). Then a body whose top-level
   `tools` is a non-empty array, sent to a model whose `ToolCalling` is `Absent`, is
   `tools-not-served` (400): a pod started without `--enable-auto-tool-choice` and a
   `--tool-call-parser` cannot answer it. No `tools` key, `"tools": []`, a `tools` that is not
   an array and any `tools` below the top level are relayed unchanged; a body naming `tools`
   twice at its top level is refused when either is a non-empty array. Nothing so far asks for
   a target, so a refused request never wakes a pod.
6. The top-level `model` value becomes the upstream name, written as a JSON string. On the
   messages wire only, `output_config.effort` and `chat_template_kwargs.reasoning_effort` equal
   to `high` or `max` become `xhigh`, because the served chat template rejects both (llmgw
   `src/lib.rs:470-504` for `high`): the Qwen3.8 template accepts `xhigh`, `medium` and `low`
   and raises on any other effort (`Qwen/Qwen3.8-27B-FP8` `chat_template.jinja` lines 46-49 at
   Hugging Face commit `017b9c7`), which fails the turn, and `xhigh` is its deepest. `low`,
   `medium` and `xhigh` reach the target unchanged, as does any other value, and both fields on
   the chat and responses wires. On the messages wire only, a body that names no effort (no
   `effort` member in a top-level `output_config` object and no `reasoning_effort` member in a
   top-level `chat_template_kwargs` object, whatever such a member's value), sent to a model
   that declares a `ReasoningEffort` (`RelayModel::with_reasoning_effort`: `low`, `medium` or
   `xhigh`), reaches the target with `output_config.effort` set to it. A top-level
   `output_config` object gains `"effort":"<v>"` as its last member (the last such object when
   the key is named twice), and a body without one gains `"output_config":{"effort":"<v>"}` as
   its last top-level member; a body whose top-level `output_config` is not an object gains
   nothing. A body that names an effort keeps the mapping above and never the declared value.
   The binary declares a hosted model's `reasoning_effort` and never a Runpod model's: a
   hand-started server's chat-template defaults are not the gateway's to set, and the Qwen3.8
   template defaults to `xhigh` when a request names no effort, which vLLM v0.27.1 hands it as
   `reasoning_effort` from `output_config.effort` (`vllm/entrypoints/anthropic/serving.py` lines
   513-514); a Runpod pod is already started with its model's `reasoning_effort` as the
   template's default. Every other byte reaches the target unchanged: llmgw re-serialises the
   document with sorted keys, llm-gateway replaces those values in place and inserts only that
   member.
7. `acquire`, which hands out a target or a `TargetRefusal`: `Unavailable` is
   `target-unavailable` (503), and `ColdStart`, the model's target still starting when the
   request's hold budget passed, is `model-cold-start` (503) with `retry-after: 30`, as llmgw's
   `model_cold_start` (row W6). It is the only refusal that carries `retry-after`. The embedding
   holds the request while its target starts and owns the hold budget; the Runpod composition
   asks its pool every 500 ms for up to the model's `request_hold_seconds` (rows L6 and K28,
   `docs/hosting.md`). Then, on the messages wire only, the output clamp. A model's window `w`
   is its `max_model_len`, else its `context_window`, else it has none: the binary declares a
   Runpod model's `max_model_len`, what `vllm serve` is started with, and a hosted model's
   `context_window` and never its `max_model_len`, so a hosted model without a `context_window`
   has no window. For a model with a window and a body whose top-level `max_tokens` is one
   non-negative JSON integer (digits only, named once at the top level), the gateway first sends
   `POST /v1/messages/count_tokens` to the target `acquire` handed out, over a connection of its
   own, with the head and credential described below and the body step 6 wrote; vLLM v0.27.1
   answers it `{"input_tokens":n,"context_management":{"original_input_tokens":n}}`
   (`vllm/entrypoints/anthropic/api_router.py` lines 95-133, `serving.py` lines 1020-1025), of
   which only the top-level `input_tokens` is read. The count has its own deadline,
   `GatewayConfig::count_timeout` (ten seconds by default), from the moment its connection is
   open: a count not answered by then fails, so the target's own read timeout, which may be far
   longer, never holds the request; the abandoned count's connection closes when the target
   answers or its own timeout ends it. A
   2xx answer whose body is one JSON object with one top-level non-negative integer
   `input_tokens` = `n` decides: `w - n < 1` is `context-exceeded` (400) and nothing is
   relayed; a `max_tokens` above `w - n` is rewritten in place to `w - n`, and one at or below it
   is unchanged. The refusal's message starts `prompt is too long`, which Claude Code (2.1.301)
   classes as a prompt that is too long and answers by compacting its conversation, where a vLLM
   500 for a request past its window ends the turn. A count that fails (no connection, a request
   that cannot be written, an answer head that never arrives or is malformed, a status outside
   2xx, an answer body longer than `request-head-bytes`, a body without such an
   `input_tokens`, or an answer not complete by the count's deadline) relays the request unchanged, reports nothing to `invalidate` and counts no
   series. A body without such a `max_tokens`, a model without a window, and the chat and
   responses wires are relayed without a count. The count runs only on a target `acquire`
   handed out, so it never wakes a pod the relay would not have woken. Then `POST` to the same
   path at the
   target with `host` set to the target's authority and a `content-length`. A target with a
   `BasePath` (`RelayTarget::base_path`: a hosted endpoint whose `base_url` names a path) is
   asked on that path joined to the wire path instead, the base path replacing the wire path's
   leading `/v1`, and so is its count: `/api/v1` asks `/api/v1/chat/completions` and
   `/api/v1/messages/count_tokens`, `/v1` asks the wire path itself. `host` stays the
   authority. A `BasePath` is `/` and 1 to `base-path-segments` segments of `A-Z a-z 0-9 - . _ ~`,
   none of them empty, `.` or `..`, at most `base-path-bytes` long with one final `/` dropped,
   refused as `BasePathError::Length`, `NotAPath` and `DotSegment`; so it can neither end the
   request line nor leave the prefix it names. A target with static headers
   (`RelayTarget::headers`: a hosted endpoint's `headers`, such as `anthropic-version`) is sent
   each as `<name>: <value>` right after `host`, in the order given (the binary gives name
   order), on the request and on its count. A `StaticHeader` name follows the
   `CredentialHeader` rule below and is held lowercase; its value is 1 to
   `static-header-value-bytes` of 0x20-0x7e with no leading or trailing space; refused as
   `StaticHeaderError::NameLength`, `NameNotAToken`, `Reserved`, `ValueLength` and
   `ValueNotPrintable`. So no static header can end its line, replace or repeat a header the
   gateway writes, or carry `authorization`. A target whose
   `bearer` is a `TargetBearer` is sent `authorization: Bearer <key>` (row B8): for a Runpod pod,
   the model's vLLM key. A target whose `credential_header` also names a `CredentialHeader`
   (an `api-key` hosted endpoint) is sent `<header>: <key>` instead, and no `authorization`; a
   `CredentialHeader` is one RFC 9110 token of at most `credential-header-bytes`, held
   lowercase, and never a header the gateway writes itself or one that frames a message
   (`host`, `authorization`, `content-type`, `content-length`, `connection`,
   `transfer-encoding`, `te`, `trailer`, `upgrade`, `expect`, `keep-alive`,
   `proxy-authorization`, `proxy-connection`), refused as `CredentialHeaderError::Length`,
   `NotAToken` and `Reserved`. A target without one is sent no `authorization`. The binary
   never sends a hosted endpoint's credential in cleartext to a remote host: a `bearer` or
   `api-key` endpoint whose base URL is `http` and whose host is not a loopback literal
   (`127.0.0.0/8`, `::1`) or `localhost` is refused when the document is loaded
   (`config:value`, naming the endpoint and quoting no credential). The head is the
   gateway's own and forwards no header of the client's request, so the owner's credential never
   reaches a target. A `TargetBearer` follows the owner material's rules, refused as
   `bearer:empty`, `bearer:too-large` (over `owner-credential-bytes`) and
   `bearer:not-printable-ascii`, so it is one token in the line it is written into; it is
   redacted in `Debug` and overwritten when dropped. The embedding decides what a stop does
   to `acquire`: the Runpod composition hands out a pod that is ready now and answers
   `target-unavailable` at once instead of starting or awaiting one (`docs/hosting.md`).
8. A connection that fails, a request that cannot be written, an answer head that never arrives
   or is malformed, and a `502`, `503` or `504` answer each report the target to `invalidate` by
   its authority and answer `upstream-failed` (502). Naming the authority lets the pool drop the
   endpoint only if it is still the current one. Any other status, `500` and `4xx` included, is
   the model's own answer and is relayed.
9. The target's status, its `content-type` and its decoded body bytes are relayed as they
   arrive, as `transfer-encoding: chunked` with `cache-control: no-store`. The gateway relays
   answer bodies unchanged and does not rewrite them, so whatever an upstream puts in its answer
   body reaches the client. An answer cut short on either side ends without its last chunk, so
   the client sees it cut. An HTTP/1.0 client may not be sent a chunked answer (RFC 9112
   section 6.1), so it gets the bytes unframed and the end of the answer is the end of the
   connection. The target is held until the last byte
   is relayed, then released. Every head the target sends, interim `1xx` heads included, comes
   out of one `request-head-bytes` budget, and so does its trailer section, so a target cannot
   grow the gateway's memory.

A refused relay closes gracefully: the refusal is written, the write side closed, and what the
client still sends is read and discarded (up to `request-body-bytes`, within `read_timeout`), so
a reset cannot destroy the refusal before the client reads it.

### Claude Code on the messages wire

What Claude Code 2.1.293 sends to a model id it does not recognise, such as a gateway alias, meets
the pinned vLLM (`vllm/vllm-openai:v0.27.1`) through this relay as below. Each point is settled
from Claude Code's published [gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol)
and vLLM's source at tag `v0.27.1`
(`https://github.com/vllm-project/vllm/blob/v0.27.1/`, paths below relative to it), both read
2026-10-15, not from a recorded session; `docs/verification/2026-10-15-claude-code-wire.md` lists
the sources and what a live record still has to show. The cases are the `claude_code_*` tests of
`crates/llm-gateway/tests/wire_relay.rs`, over fixtures authored from those sources, and the
`claude-code-*` scenarios.

- **`HEAD /api/hello`.** Claude Code's connection-warming probe has no route here. Without the
  owner credential it is `credential-absent` (401), and with it `path-unknown` (404), each
  without a body, as any other path off the public surface; no target is asked for. The protocol
  page names it startup traffic a gateway "can reject without breaking anything", and vLLM has
  no such route either.
- **`thinking`.** Relayed unchanged, `{"type":"adaptive"}` included. vLLM v0.27.1's messages
  request model declares no `thinking` field and drops unknown keys
  (`vllm/entrypoints/anthropic/protocol.py:118-178`), so every `thinking` value is ignored
  there, never refused. What turns the served model's thinking on is the effort:
  `output_config.effort` becomes vLLM's `reasoning_effort`
  (`vllm/entrypoints/anthropic/serving.py:513-514`), and any effort but `none` sets the chat
  template's `enable_thinking` (`vllm/entrypoints/openai/chat_completion/protocol.py:564-573`).
- **Effort.** Claude Code sends `low`, `medium`, `high`, `xhigh` or `max`; vLLM admits all five
  (`vllm/entrypoints/anthropic/protocol.py:114`) and hands the value to the chat template, so
  step 6 maps the two the template rejects.
- **Structured output.** `output_config.format` is relayed unchanged. vLLM v0.27.1 serves
  `{"type":"json_schema","schema":…}` as structured output, its `response_format`
  (`vllm/entrypoints/anthropic/serving.py:505-512`), and ignores the deprecated top-level
  `output_format`, which it does not declare; the gateway relays that unchanged too. Claude Code
  stops sending the format under `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1`.
- **`POST /v1/messages/count_tokens`.** Not relayed, although vLLM v0.27.1 serves it
  (`vllm/entrypoints/anthropic/api_router.py:95-96`): with the owner credential, a `POST` to a
  path that is not a wire path is `method-not-allowed` (405), and no target is asked for. The
  protocol page says Claude Code falls back to estimated counts when the endpoint is missing;
  whether it treats a 405 as it treats a 404 is for the live record.
- **Headers.** `anthropic-version` and `anthropic-beta` are dropped, as every header of the
  client's request is (step 7). vLLM v0.27.1 reads neither: its messages router and serving code
  read no request header (`vllm/entrypoints/anthropic/api_router.py`,
  `vllm/entrypoints/anthropic/serving.py`). The protocol page's advice to forward them verbatim
  is written for an Anthropic upstream. The query of `POST /v1/messages?beta=true` is discarded
  like any other (step 7 asks the target on the wire's own path).
- **The answer.** vLLM's event stream, thinking blocks, `signature_delta` and `tool_use` blocks
  included, reaches Claude Code byte for byte (step 9). vLLM writes no `ping` event, and the
  gateway adds none.

## Counters and usage records

Every request to a wire path that passed owner authentication makes one `UsageRecord` once its
answer or refusal is written, before the connection is closed: the alias of the relayed model the
body named (absent when it named none), the wire, its `Disposition` (`Relayed`, `Refused` or
`UpstreamFailed`), the refusal code when it was not relayed, the status the client received, the
status the target answered with (absent when no target answered), the decoded body bytes of a relayed answer (0 for a refusal) and the milliseconds from the end of the
request head to the last byte written. The token counters and the model the target reported are
always absent for now. An unauthenticated or shed request makes no record. The record feeds the
counters, then leaves the crate through the `UsageRecords` port the embedding implements
(`Relay::with_records`); the binary writes each as one `tracing` event at `info`.

`GET /metrics` serves llmgw's 13 counters under their llmgw names, so a scrape configured for
llmgw keeps working. It takes the owner credential and readiness like inspection, and answers
`content-type: text/plain; version=0.0.4; charset=utf-8`. The text is rendered inside this crate,
which takes no dependency for it.

| Series | Labels | Counts |
| --- | --- | --- |
| `llmgw_inference_requests_total` | | every usage record |
| `llmgw_upstream_failures_total` | | every record no target answered: unreachable, or closed without an answer |
| `llmgw_instruction_views_total` | | every `GET /` and `HEAD /` answered `200` ([the public routes](#the-public-routes)) |
| `llmgw_pod_starts_total` | | pod creates the embedding counts (`Metrics::count_pod_start`) |
| `llmgw_pod_start_failures_total` | | pods the embedding counts as failed to start |
| `llmgw_pod_reaps_total` | | deployments and pods the embedding's cleanup pass stopped |
| `llmgw_endpoint_invalidations_total` | | each target reported to `invalidate` |
| `llmgw_cold_start_wait_seconds_total` | | seconds `acquire` took for a request held for a starting target |
| `llmgw_route_requests_total` | `model`, `wire` | records naming a relayed model on a wire it declares |
| `llmgw_route_refusals_total` | `model`, `wire` | the same, refused or no target answered |
| `llmgw_route_upstream_status_failures_total` | `model`, `wire` | the same, the target answered outside 2xx: a 4xx or 500 relayed, or a 502, 503 or 504 answered `upstream-failed` |
| `llmgw_route_cold_start_holds_total` | `model`, `wire` | requests held for a starting target: the target says it held them (`RelayTarget::held`), or `acquire` answered `model-cold-start` |
| `llmgw_route_response_bytes_total` | `model`, `wire` | the records' response bytes |

Every (model, wire) pair a relayed model declares is registered at zero when the relay is
composed, so an uncalled route reads 0 instead of being absent; a request for another pair feeds
the process-wide series only. A gateway composed without a relay serves the process-wide series
alone. An embedding that counts pod events shares one `Metrics` with the relay
(`Relay::with_metrics`). The upstream and status series count as llmgw does (llmgw
`src/lib.rs:598-629` at `048ebd8`). The specification is `spec/domains/telemetry.yaml`.

## Refusals

A refusal names its own stable code and a message this crate wrote. It quotes nothing the caller
sent: a rejected credential is never echoed back, and no upstream response text reaches a
refusal. A target's own answer is relayed, not refused, and is the target's text.

```json
{"error":{"code":"credential-absent","message":"no owner credential was presented"}}
```

| Code | Status | Message |
| --- | --- | --- |
| `body-incomplete` | 400 | the request body ended or stalled before it was complete |
| `body-not-allowed` | 400 | the inspection surface accepts no request body |
| `credential-absent` | 401 | no owner credential was presented |
| `credential-malformed` | 401 | the owner credential is not a bearer token |
| `credential-rejected` | 401 | the presented owner credential was rejected |
| `body-not-json` | 400 | the request body is not one JSON object |
| `body-too-large` | 413 | the request body exceeds its byte bound |
| `context-exceeded` | 400 | prompt is too long: the input leaves no room for output in the model context window |
| `method-not-allowed` | 405 | this path does not accept that method |
| `model-absent` | 400 | the request body names no model |
| `model-cold-start` | 503 | the model is still starting; ask again after the retry-after delay |
| `model-unknown` | 404 | no such model |
| `overloaded` | 503 | the gateway is already serving its maximum concurrent requests |
| `path-unknown` | 404 | no such gateway resource |
| `request-malformed` | 400 | the request is not well-formed HTTP |
| `request-too-large` | 431 | the request head exceeds its byte bound |
| `route-unknown` | 404 | no such route alias |
| `target-unavailable` | 503 | no model target is available |
| `tools-not-served` | 400 | the model does not serve tool calls |
| `unavailable` | 503 | the gateway is not ready to serve inspection |
| `upstream-failed` | 502 | the model target could not be reached or failed; the next request asks for a replacement |
| `wire-not-served` | 400 | the model is not served on this wire |

This table is the published contract, and it is checked rather than maintained by hand.
`RefusalCode`, `RefusalCode::ALL`, `wire`, `status` and `reason` are generated from a single
list by the `refusal_codes!` macro, so a variant cannot exist without all five — that is a
compile error, not a test. Two runtime checks close the rest: every code must be named in this
table with **this exact status and this exact message**, and every code must have a request in
the crate's own suite that actually provokes it, enforced by an exhaustive match. A row here
that names a code the crate cannot emit fails too.

## Lifecycle

`Gateway::bind` starts listening immediately and starts **not ready**, so a readiness probe
cannot see the gateway before the embedding has finished composing it. `mark_ready` opens
inspection.

`begin_drain` reports unready while continuing to serve: a load balancer removes the gateway from
rotation and requests that arrive meanwhile still succeed. **A drain cannot be undone.**
`mark_ready` does not clear it, so an embedding that re-marks readiness on a timer cannot return
a draining gateway to rotation.

`shutdown` then stops accepting, lets every accepted connection finish, and returns a
`ShutdownReport` whose `completed` equals its `accepted` and whose `in_flight_at_signal` records
what was still running when the stop was signalled. Readiness flips to unready at the signal, but
**inspection keeps being served until the last accepted connection has finished**: no connection
the report counts as completed was refused *because of the stop*, so the serving flag is cleared
after the accepted connections are joined, never before. `completed` counts every accepted
connection, including ones refused on their own merits — an absent credential, an unknown path —
which the stop has nothing to do with. For inspection, how long that can take is
bounded by twice `read_timeout`: one deadline covers the whole request head and another the whole
response, so a peer that trickles its head or stops reading its answer cannot hold the stop. A
relay is not bounded that way, because a stream lasts as long as the model talks: its head and its
body each have one `read_timeout` deadline, each chunk written to the client has its own, and
reading the target is bounded by the timeouts of the connection the target opened. A stop waits
for a relayed stream to end. Dropping the handle without a shutdown still stops the listener, but
reports nothing.

## Bounds

Every bound the crate enforces, with the value it enforces. The constant column names where
each one lives, so this table can be compared against the source rather than against another
list written by hand.

| Bound | Constant | Value |
| --- | --- | --- |
| `owner-credential-bytes` | `MAX_TOKEN_BYTES` | 4096 |
| `shared-secret-bytes` | `MIN_SHARED_SECRET_BYTES` | 32 |
| `label-bytes` | `MAX_LABEL_BYTES` | 256 |
| `credential-header-bytes` | `MAX_CREDENTIAL_HEADER_BYTES` | 64 |
| `base-path-bytes` | `MAX_BASE_PATH_BYTES` | 256 |
| `base-path-segments` | `MAX_BASE_PATH_SEGMENTS` | 32 |
| `static-header-value-bytes` | `MAX_STATIC_HEADER_VALUE_BYTES` | 1024 |
| `targets-per-route` | `MAX_TARGETS_PER_ROUTE` | 64 |
| `routes` | `MAX_ROUTES` | 4096 |
| `digest-characters` | `DIGEST_CHARACTERS` | 64 |
| `request-head-bytes` | `DEFAULT_MAX_HEAD_BYTES` | 8192 |
| `concurrent-requests` | `DEFAULT_MAX_CONCURRENT_REQUESTS` | 64 |
| `request-body-bytes` | `MAX_REQUEST_BODY_BYTES` | 33554432 |

Three checks, and the third is the one that was missing: the table's **constant** column is
compared against every numeric constant the crate's own source declares, so a bound that exists
in the source and in neither the table nor the measurement fails
(`every_numeric_bound_in_the_source_is_published`); the table's **bound** column is compared
against the measurement set; and every row's **value** is exercised at the number itself and at
one past it, through the public API or over a socket, never by reading a configuration field
(`every_published_bound_flips_at_exactly_the_published_number`). A constant that is genuinely
not a bound has to be named in the exemption list in `tests/gateway.rs` with a reason, rather
than being invisible.

`owner-credential-bytes`, `label-bytes`, `credential-header-bytes`, `base-path-bytes`,
`base-path-segments`, `static-header-value-bytes`, `targets-per-route`, `routes`,
`request-head-bytes`, `concurrent-requests` and `request-body-bytes` are maxima; `shared-secret-bytes` is a minimum;
`digest-characters` is an exact length. `request-head-bytes` and `concurrent-requests` are
`GatewayConfig` defaults an embedding may override; they and `request-body-bytes` (32 MiB, as
llmgw) are measured by enforcement over a real socket rather than by reading a field.

The delivery of a request head is bounded separately by `GatewayConfig::read_timeout`, a timeout
rather than a size bound, so it is not in this table. It is one deadline for the whole head, not a
timeout per read, and the response is written under a second deadline of the same length. It
defaults to ten seconds, and its enforcement is measured with a short configured value by
`a_stalled_head_is_refused_when_the_read_timeout_expires`,
`a_trickled_head_is_refused_when_the_read_timeout_expires` and
`a_peer_that_never_reads_its_answer_cannot_hold_the_stop`.

The output clamp's count is bounded by `GatewayConfig::count_timeout`, also a timeout rather than
a size bound: one deadline for writing the count and reading its whole answer, from the moment its
connection is open. It defaults to ten seconds (`DEFAULT_COUNT_TIMEOUT_SECONDS`), and its
enforcement is measured with a short configured value by
`clamp_a_count_that_never_answers_relays_the_request_once_its_deadline_passes`.

`COLD_START_RETRY_AFTER_SECONDS` (30) is the `retry-after` a `model-cold-start` refusal carries. It
is a hint to the client rather than a limit the gateway enforces, so it is not in this table
either; `w6_a_model_still_starting_past_its_hold_budget_is_model_cold_start_with_retry_after_30`
in `tests/wire_relay.rs` measures the header, and
`every_refusal_code_has_a_request_that_provokes_it` checks that no other refusal carries one.

## Verification

The [verification record](verification/gateway.md) states the measured coverage, the mutations
that were shown to kill named tests, and the qualification boundaries that remain.
