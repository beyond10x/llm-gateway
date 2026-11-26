# The authenticated single-owner gateway

`llm-gateway` admits exactly one authenticated owner and lets that owner read the routes a
deployment serves. This milestone is authentication, liveness, readiness, graceful shutdown and
read-only route inspection, the relay of three model-call wires to a target the embedding
hands out ([below](#the-relay)), and the port through which a translator the embedding composes
answers a wire the model's upstream does not speak ([Translation](#translation)). The crate
itself translates no protocol, and multi-tenant accounts and quotas are not in it; it contains
nothing that could do them.

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
| `POST` | `/v1/models/{alias}/wake` | owner | Starts the model's pod and answers `202` at once with its status ([the lifecycle routes](#the-lifecycle-routes)). |
| `GET`, `HEAD` | `/v1/models/{alias}/status` | owner | The model's phase and profile; starts nothing. A hosted model's or route's is a probe of its endpoint. |
| `GET`, `HEAD` | `/v1/status` | owner | Every relayed model's kind, wires, status and whether a call is answered now ([the status list](#the-status-list)). |
| `GET`, `HEAD` | `/v1/calls/{id}` | owner, as `GET /v1/launch` takes it | The record of the model call whose answers carried `x-llm-gateway-call: {id}` ([call ids and call records](#call-ids-and-call-records)). |
| `POST` | `/v1/chat/completions` | owner, or none under `IngressAuth::None`, or a reviewed service account under `IngressAuth::ServiceAccountToken` | The `chat` wire, relayed ([the relay](#the-relay)). |
| `POST` | `/v1/responses` | owner, or none under `IngressAuth::None`, or a reviewed service account under `IngressAuth::ServiceAccountToken` | The `responses` wire, relayed. |
| `POST` | `/v1/messages` | owner, or none under `IngressAuth::None`, or a reviewed service account under `IngressAuth::ServiceAccountToken` | The `messages` wire, relayed. |

An embedding may bind a second, metrics-only listener beside this surface
([scraping the counters](#scraping-the-counters)); nothing in this table is reachable on it.

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
`request-malformed`. A CR, LF or NUL inside a line is refused as soon as it arrives, so a head
holding one is `request-malformed` at any length, never `request-too-large` or left to
`read_timeout`. A repeated header is `request-malformed`, and a repeated `authorization` is
`credential-malformed`, except where the request never reads it: a probe ignores every repeat,
and a public route every repeat but `content-length`, `transfer-encoding`, `host` and
`user-agent`. The owner presents
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

DECIDED 2026-11-04 (story:live-dashboard). `GET /llms.txt` is public too: Markdown naming the
gateway's origin, its dashboard and metrics, and the launcher commands
(`b10x-llm-gateway claude`, `b10x-llm-gateway codex`) for the models that parse tool calls; a
model's upstream name is written only on a gateway open on loopback. `GET /` for a browser
(`user-agent` starting `mozilla/`) is that same Markdown inside the dashboard's page, so the two
cannot disagree. `GET /v1/launch` is what the launchers read: `llm-gateway.launch/1` JSON with the
origin and, per relayed model, its alias, prefix, whether it forwards the client's own credential,
whether Claude Code and Codex reach it, its context and auto-compact windows, its `/model` label
and its phase. It reads each pod model's status, so it takes the owner credential (none on a
gateway open on loopback) and is not a public route. It asks no endpoint: a hosted model's, a
route's and a Bedrock model's `phase` is `null` there, because their status is a probe of the
endpoint (or none), and the read that probes is `GET /v1/status` ([the status
list](#the-status-list)) or `GET /v1/models/{alias}/status`.

`GET /v1/models` is `{"object":"list","data":[…]}`, one entry per model in alias order:
`{"id":<alias>,"object":"model","owned_by":"llm-gateway","max_model_len"?:<n>,"wires":[…]}`.
`max_model_len` is what `RelayModel::with_max_model_len` declared, and absent, never `0`, when
nothing was. The binary declares a Runpod model's as the smallest `max_model_len` among its
profiles.

`GET /` chooses its answer from the lowercased `user-agent`, by the first rule that holds:
containing `codex`, a Codex `config.toml` profile per model (`text/plain; charset=utf-8`);
containing `claude` or `anthropic`, Claude Code's environment variables per model (the same
type); starting with `mozilla/`, the `/llms.txt` page above (`text/html; charset=utf-8`); anything else, or no
`user-agent`, plain text with the llm catalog lines a Loom run needs. A client gets a profile for
a model only when the client's requests reach the model on the client's wire (Codex `responses`,
Claude Code `messages`, Loom `chat`) and the model parses tool calls; each profile names the
model's alias and the `context_window` from `RelayModel::with_context_window`. The requests reach
the model relayed on a wire it declares and its target takes verbatim (a Bedrock model, which
declares `messages`, takes none), or translated on another wire when the relay has a translator
that admits that client to the model (`Translator::admits`, default `false`). The relay asks once
per model and wire it does not relay, in `Relay::with_translator`, never per request. The
binary's translator prepares the client's recorded first turn for the model exactly as it
prepares a request ("Translation", "The request", steps 2 to 4), and admits the client when llm
carries it: Codex 0.160.0's captured turn
(`crates/llm-gateway-cli/src/admission/codex/first-turn.json`, effort `high`, no output limit)
and Claude Code's authored one
(`crates/llm-gateway-cli/src/admission/claude-code/1-tool-turn.request.txt`, effort `max`, which
reaches a chat or responses upstream as `xhigh`, and `max_tokens` 32000). So a routes-document
route with the default capabilities, whose efforts stop at `high` and whose output limit is
16384, is profiled for Codex and not for Claude Code. No Loom turn is recorded, so a Loom run is
profiled only for models that declare `chat`. `GET /` thus names only models a client's requests
reach without a `translation-*` refusal. Admission covers the first turn only, and every
translated path refuses Claude Code's structured output (`output_config.format`,
`translation-unsupported`; see "Structured output and images" below), which Claude Code sends in
side requests unless
`CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1`
(`crates/llm-gateway/tests/fixtures/claude-code/3-structured-output.request.txt`). That the
variable stops it is what Claude Code's published gateway protocol says
(`docs/verification/2026-10-15-claude-code-wire.md`); it has not been observed against a live
Claude Code client. So a Claude Code block for a model its requests reach only translated ends
with `# Reached by translation, which refuses structured outputs from Claude Code.` and
`export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'`; a block for a model they reach relayed
keeps structured output. No comment line of a block holds a quote character, so a block pasted
into an interactive shell that reads `#` as a command opens no quote that swallows the export
after it. When no model gets a client's profile, the answer ends with two comment lines naming
only what was checked. For Codex and Claude Code:
`# No model of this gateway serves <client>: none with tool calling parsed relays the` and
`# <wire> wire, or admits the recorded first turn of <client> translated.` For a Loom run,
whose turn is not recorded although the gateway translates its chat requests:
`# No model of this gateway serves Loom: none with tool calling parsed relays the` and
`# chat wire, and no Loom turn is recorded to try a translated one.` Neither says that no model
declares the wire, which a translated-only model declaring it and left out would contradict.
The base URLs use `http://` and the request's `host`
when it is only letters, digits, `.`, `-`, `_`, `:`, `[` and `]`, and the
listener's address otherwise. The owner token is named as the setting that carries it, never
given. Each `GET /` or `HEAD /` answered `200` counts one `llmgw_instruction_views_total`.

## The lifecycle routes

A gateway composed with a relay serves two owner routes per relayed model, so the owner can start
a model ahead of demand and read where its start is without sending a model call. `{alias}` is
the rest of the path between `/v1/models/` and the last segment, compared byte for byte with the
relayed models' aliases (nothing is percent-decoded; an empty alias is no lifecycle path). They
are not wire paths, so they take the owner credential under `IngressAuth::None` too, and they
are decided after the probes, shedding and authentication, in this order, before the target
source is asked anything:

1. A method the route does not take is `method-not-allowed`, with `allow: POST` for the wake and
   `allow: GET, HEAD` for the status.
2. A declared body is `body-not-allowed`, and before `mark_ready` the route is `unavailable`.
3. An alias no relayed model has is `model-unknown`.
4. A model whose target source starts nothing for it is `path-unknown`: the route does not
   exist for it. `RelayTargets::wake` and `RelayTargets::status` answer `None` by default. The
   binary's wake answers `None` for a hosted endpoint, an Ollama model and a route of the routes
   document, which are relayed upstreams with no lifecycle; its status answers `None` only for a
   Bedrock model.

Then `POST /v1/models/{alias}/wake` asks the source to start the model's target as a request's
first ask for it would (`RelayTargets::wake`) and answers `202` at once with the status the
source reports after that ask; it never waits for the target to serve. While a start is in
progress, or the target is ready, a wake starts nothing more and answers `202` the same way. A
refusal is the one `acquire` would have earned: `target-unavailable`, `spend-ceiling` (503,
without `retry-after`) or `model-cold-start` (with `retry-after: 30`). A wake is never
`model-stopped`: in the binary it is the only way a Runpod model without `start_on_request` gets
a pod, and that model's requests are `model-stopped` until it has one.
`GET /v1/models/{alias}/status` asks the source for the status (`RelayTargets::status`), which
starts nothing, and answers `200`; `HEAD` answers the same without a body.

Both answers are `application/json`, a `ModelStatus` in this key order, a value the source did
not report absent, never empty:

```json
{"model":"small","phase":"starting","sub_phase":"pending","profile":"l40s-32k"}
```

`phase` is a `ModelPhase`, named by what a request for the model would meet: `stopped` (no pod
is live; a request starts one), `starting` (a request is held while the pod starts), `ready`
(the pod was last observed serving) and `stopping` (a previous pod owes a stop not yet
confirmed; nothing starts meanwhile) and `unreachable` (the model's endpoint did not answer its
probe as serving; a call fails until it does). `sub_phase` is the provider's own word for the
pod while it starts, or for an `unreachable` model why: `no-answer` (no final status inside the
probe's deadline: refused, closed, untrusted TLS or silent), `credential-refused` (401 or 403
to the model's own credential), `status-<code>` (any other non-2xx status, three digits) or,
for an Ollama model, `model-not-pulled` (its server does not list it); a `stopped` Ollama model
is `not-loaded`.
`profile` is the profile a pod was started on. Neither route makes a usage record or counts a
series of its own, and a status read never asks the source to acquire, wake or invalidate
anything. The specification is `llm-gateway.gateway.LifecycleRoute`; the scenarios are
`contracts/gateway/scenarios/wake-*` and `contracts/gateway/scenarios/status-*`.

In the binary a Runpod model's wake is one `RunpodPool::wake`, so its spend check and its
profile fallback are a request's, and the pod it starts, a spend refusal or a failed start is
counted as a request's would be (`docs/hosting.md`, "Waking a model and reading its phase").
Its status is `RunpodPool::status`, read from the hosting state machine with no provider call.

A hosted model's and a route's status is a probe of its endpoint, the one `credential set`
sends: `GET <base path>/models` with the model's credential and static headers and nothing
else. A 2xx answer is `ready`; for an endpoint whose credential is the client's own
(`auth_kind = "client"`) the probe carries no credential, and a 401 or 403 is `ready` too (the
endpoint answers, and a caller's own credential decides). Anything else is `unreachable` with
its `sub_phase`. The probe runs only when a status is read, never in the background: one probe
per model at a time (a second read waits for the first and takes its result), and a result is
reused until the reuse time has passed since its probe ended ([bounds of the status
probe](#bounds-of-the-status-probe)). Wake stays `path-unknown` for both.

An Ollama model's status (`[hosted.<alias>] kind = "ollama"`, [an Ollama
upstream](#an-ollama-upstream)) is read from its server's own lists instead, never from
`/v1/models`: `GET /api/tags`, then, only when the tags list the model, `GET /api/ps`. Each
carries `host`, the static headers, `accept: application/json` and `connection: close`, no
credential and no body, and both end within the one probe deadline, under the same lock and
reuse as a probe. An entry names the model by its `name` or its `model`; a model name without a
tag (no `:` after its last `/`) is `<name>:latest`, one with a tag itself only.

| The server answers | Status |
| --- | --- |
| no final status by the deadline, or a 2xx whose body is not one `{"models":[...]}` of at most `ollama-list-max-bytes` | `unreachable` `no-answer` |
| a status outside 2xx to either list | `unreachable` `status-<code>` |
| tags without the model | `unreachable` `model-not-pulled` |
| tags with it, the running models without it | `stopped` `not-loaded` (callable: a call loads it) |
| both with it | `ready` |

A status read sends nothing else, and wake stays `path-unknown`: the gateway starts, loads or
pulls nothing (`ollama_status_is_read_from_tags_then_ps_and_sends_nothing_else` in
`crates/llm-gateway-cli/tests/wake_status.rs`, scenarios `contracts/gateway/scenarios/ollama-*`).

### The status list

`GET /v1/status` answers one list of every relayed model, in alias order, as
`application/json`; `HEAD` answers the same without a body. It takes the owner credential, and
the method (`allow: GET, HEAD`), body and readiness checks of the lifecycle routes, all before
any status is asked. A gateway composed without a relay answers an empty list.

```json
{"format":"llm-gateway.status/1","models":[
 {"model":"a-pod","kind":"pod","wires":["chat"],"phase":"stopped","callable":true},
 {"model":"b-hosted","kind":"hosted","wires":["chat","responses"],"phase":"unreachable","sub_phase":"no-answer","callable":false},
 {"model":"c-bedrock","kind":"bedrock","wires":["messages"]}]}
```

(Wrapped here; the answer is one line.) Each entry names `model`, `kind` (`pod`, `hosted`,
`route`, `bedrock` or `ollama`), `wires` (the wires its upstream declares), the `ModelStatus` keys
`phase`, `sub_phase` and `profile`, and `callable`: whether a call sent now is relayed, held on
a pod that is starting or serving, or starts the pod. `callable` is `true` for `ready` and
`starting`, and for `stopped` where a call starts the pod; a model declared with
`RelayModel::with_no_call_start` is `false` while `stopped`. The binary declares it on every
Runpod model whose call does not start its pod (no `start_on_request` under its own provider's
enforced daily cap), whose call is answered `model-stopped`. `callable` is `false` for
`unreachable`, and for `stopping`, with or without `start_on_request`: the call would be
refused, or would wait on a stop still owed, with no promise of an answer from the model. A call
with `start_on_request` to a `stopping` model meets one of three outcomes:

- The stop is confirmed within its hold: it binds to the replacement pod and is relayed, or is
  `model-cold-start` if that pod still starts when the hold passes.
- The stop is still owed when its hold passes: `target-unavailable`.
- The gateway is stopping: `target-unavailable` at once.

An Ollama model is never a pod model: `start_on_request` and `RelayModel::with_no_call_start`
do not apply to it, and its `callable` follows its phase alone, `true` for `ready` and for
`stopped` `not-loaded` (a call loads the model), `false` for `unreachable`. A model with no
status (Bedrock) has neither `phase` nor `callable`. Every status is asked side by side, so the
list is answered within one probe deadline and a second however many endpoints are silent.

## Service-account tokens

`GatewayConfig::ingress_auth = IngressAuth::ServiceAccountToken` admits a model call (a wire
path or the token count of a gateway composed with a relay) whose bearer is the owner's, as
`Bearer` does, or one the relay's `CallerReview` admits (`Relay::with_caller_review`). The
owner's bearer is compared first and is never shown to the review. Any other bearer is handed
to the review as a `CallerToken` (redacted in `Debug`, overwritten on drop, 1..=4096 bytes of
printable US-ASCII), which answers one of three things:

- `Review::Admitted(principal)`: the call goes on to the relay order below, and its usage
  record carries the principal, `system:serviceaccount:<namespace>:<name>` of at most
  `principal-bytes` (`Principal::new` refuses anything else).
- `Review::Rejected`: `credential-rejected` (401), as a wrong owner bearer is.
- `Review::Unavailable`: `credential-review-unavailable` (503). A gateway composed with no
  review answers this to every caller but the owner.

No `authorization` is `credential-absent`, and one that is not a single bearer token
`credential-malformed`, as under `Bearer`. A refused call makes no usage record. Every other
path, `/metrics`, `/v1/routes`, `/dashboard`, the lifecycle routes and an unknown path
included, takes the owner's bearer alone and never asks the review, so a reviewed token opens
nothing but the model calls. The principal comes from the review and from nothing the request
carries: no header names it. Under `Bearer` and `None` a composed review is never asked.

The review is the embedding's, behind the port, so this crate opens no connection for it. The
binary's review asks the cluster's TokenReview API; its keys are the deployment document's
`[token_review]`, and its own bounds are in
[bounds of the service-account review](#bounds-of-the-service-account-review).

## The relay

`Gateway::bind_with_relay` composes the gateway with a `Relay`: the relayed models and an injected
`RelayTargets`. A `RelayModel` is an alias, the model name its target expects, a non-empty set
of distinct `Wire`s (`relay:no-wire`, `relay:repeated-wire`; two models with one alias are
`relay:duplicate-model`; an empty name prefix is `relay:empty-prefix`), its `ToolCalling`, `Parsed` or `Absent`, which is `Absent` unless
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
   missing, malformed, repeated or wrong credential is no refusal, and it is never forwarded.
   Every other
   path still authenticates, so `/metrics`, `/v1/routes` and an unknown path stay closed and the
   surface still cannot be enumerated. An embedding with no owner at all composes the gateway
   with `NoOwnerVerifier`, which accepts no credential: the owner-only paths are then
   `credential-absent` without one and `credential-rejected` with a bearer token (anything else
   is `credential-malformed`, as with a secret). The binary does exactly
   this for `ingress_auth = "none"` without an `owner_secret_file`, and says so on standard
   error before it listens. Under `IngressAuth::ServiceAccountToken` a wire path of a gateway
   composed with a relay takes the owner's bearer, or a bearer the relay's `CallerReview`
   admits ([service-account tokens](#service-account-tokens)); every other path takes the
   owner's bearer alone.
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
5. The model must declare the path's wire, or the relay must have a `Translator`
   (`Relay::with_translator`), which then takes the request ([Translation](#translation));
   without one another wire is `wire-not-served`. Then a body whose top-level
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
   `model_cold_start` (row W6). It is the only refusal that carries `retry-after`.
   `SpendCeiling`, starting the model's pod would pass a spend ceiling of its provider account
   or that account's spend is unknown, is `spend-ceiling` (503), answered at once and without
   `retry-after`; its message names neither the ceiling nor the amount. `Stopped`, the
   model's pod is not running and a request does not start it, is `model-stopped` (503),
   answered at once and without `retry-after`; its message says the owner wakes the model.
   The embedding holds the request while its target starts and owns the hold budget; the
   Runpod composition asks its pool every 500 ms for up to the model's `request_hold_seconds`
   (rows L6 and K28, `docs/hosting.md`). It starts a pod for a request only for a model whose
   `start_on_request` is `true` and whose provider declares `daily_cap_usd`; for any other
   model, the default, a request to a pod that is stopped or stopping is `model-stopped` and
   starts nothing, a request to a pod the owner's wake started is held on it as above, and a pod
   that fails while a request is held, or fails a relay, starts no replacement: the next
   request is `model-stopped` until the owner wakes the model (story:a-call-starts-no-pod).
   Once the gateway starts to stop, a request starts and waits for nothing, with or without the
   key: without a ready pod it is `target-unavailable`, not `model-stopped`. Then, on the messages wire only, the output clamp. The window `w` is the
   `max_model_len` the target handed out states (`RelayTarget::max_model_len`), else the model's
   `max_model_len`, else its `context_window`, else it has none: the binary declares a Runpod
   model's `max_model_len`, what `vllm serve` is started with, and a hosted model's
   `context_window` and never its `max_model_len`, so a hosted model without a `context_window`
   has no window. A Runpod pod states the `max_model_len` of the profile it was started on, so a
   call served by a fallback profile is clamped to that profile's window; a pod whose profile is
   unknown is clamped to the model's, the smallest among its profiles. For a model with a window and a body whose top-level `max_tokens` is one
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
   gateway writes, or carry `authorization`; one named like the target's own credential header
   is not written, so the head never carries that header twice. A target may also answer one
   `StaticHeader` for the request's caller (`RelayTarget::caller_header`, asked once per
   request with a `Caller`: the client's `x-opencode-session` when it is 1 to
   `caller-session-bytes` of 0x21-0x7e, and the address its connection came from, an IPv4
   address mapped into IPv6 read as IPv4). It is written where the static header of its name
   stands, or after the static headers when none does, on the request and on its count, so the
   target is sent one header of that name; a session that breaks the rule is dropped, never
   refused and never written. The binary's `OpenCode` Go endpoint answers its session here
   ([The OpenCode Go session](#the-opencode-go-session)). A target whose
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
   reaches a target; the exceptions are a valid `x-opencode-session` to an OpenCode Go
   endpoint ([the OpenCode Go session](#the-opencode-go-session)) and the client's own
   credential to a model marked `RelayModel::with_client_credentials`
   ([the client's own credential](#the-clients-own-credential)). A `TargetBearer` follows the owner material's rules, refused as
   `bearer:empty`, `bearer:too-large` (over `owner-credential-bytes`) and
   `bearer:not-printable-ascii`, so it is one token in the line it is written into; it is
   redacted in `Debug` and overwritten when dropped. The embedding decides what a stop does
   to `acquire`: the Runpod composition hands out a pod that is ready now and answers
   `target-unavailable` at once instead of starting or awaiting one (`docs/hosting.md`).
8. A connection that fails, a request that cannot be written, an answer head that never arrives
   or is malformed, and a `502`, `503` or `504` answer each report the target to `invalidate` by
   its authority and answer `upstream-failed` (502). Naming the authority lets the pool drop the
   endpoint only if it is still the current one. Any other status, `500` and `4xx` included, is
   the model's own answer and is relayed. A malformed head includes a status line, a field
   name, or a `content-type`, `content-length` or `transfer-encoding` value that is not UTF-8.
   The value of any other field, every trailer field and every chunk extension (RFC 9112
   section 7.1.1) is not read, so a byte that is not UTF-8 there (obs-text, RFC 9110 section
   5.5, which a proxy in front of a target may send) is relayed past and the target is kept.
   While the gateway waits for the answer head it watches the client every 100 ms, a read that
   does not wait: a client that has closed its connection or its sending side, typically one
   that gave up on a non-streamed answer the target is still generating, ends the wait. Bytes
   the client sent after its request, such as a pipelined next request, are read and discarded
   (at most 64 KiB a look), so a close behind them is seen too; the answer carries
   `connection: close`, so they are never served. The gateway shuts its connection to the
   target down, reports nothing to `invalidate`, writes nothing and releases the target; the
   record is the one a client gone before any answer makes
   ([Counters and usage records](#counters-and-usage-records)). The connection shut down is the
   socket the target's stream names (`RelayStream::shutdown_handle`): the binary's plain `http`
   connection and its TLS connection (an `https` endpoint, a Runpod pod) both name theirs. A
   stream that names none is held until the target answers or its read timeout ends.
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

### A streamed target

A target may ask to be sent every call streamed (`RelayTarget::streamed`); on a relay that holds
an assembler (`Relay::with_assembler`, the embedding's `StreamAssembler`) it is. The binary's
Runpod pods ask: Runpod's proxy closes a connection on which the pod sends nothing for its bound,
100 seconds by Runpod's documentation and 125 seconds by Cloudflare's Proxy Read Timeout, which
Runpod's proxy runs on. Neither page says whether the bound counts from the request, between
two bytes, or over the whole connection, so the gateway assumes the worst a long silent answer
can meet: a non-streamed answer is silent until its end, a streamed one sends its first event at
once and then one per token.

- Every call to such a target but a token count and a call to a model that authenticates the
  client itself is sent with a top-level `stream: true` (each
  `stream` member set to `true`, or the member added), and on the chat wire with
  `stream_options: {"include_usage": true}` when the body names no `stream_options`, so the last
  event carries the usage. Nothing else of the body changes.
- A call the client streamed is relayed as step 9 says.
- Of a call the client did not stream, a 2xx `text/event-stream` answer is read whole and handed
  to the assembler, which makes the one body the same events' non-streamed answer is on the
  client's wire. The client gets it with the target's status, `content-type: application/json`
  and a `content-length`; its usage is read from that body. Any other answer (a model's error,
  a JSON answer) is relayed as step 9 says.
- An event stream whose read fails before its end is the upstream cut: the head is written, the
  answer ends without its body (chunked, without its last chunk), the client sees it cut, and
  the call ends `UpstreamCut` (`FinishedCall::ending`). A `524` answer of a streamed target,
  the proxy's own answer to an origin silent past its bound, is relayed as step 9 says and ends
  `UpstreamCut` too.
- A stream the assembler makes no answer of (no end event, an `error` event, an event that does
  not read) or that passes its bound is `upstream-failed` (502); the target is not invalidated.

The binary's assembler (`EventAssembler`, `crates/llm-gateway-cli/src/assemble.rs`) reads at
most `MAX_ASSEMBLED_BYTES`, 16 MiB of events, per answer. On the chat wire it merges every
chunk's delta (content and reasoning concatenated, tool calls joined by their index, the last
finish reason, the logprobs content joined, the usage chunk's usage) and needs `[DONE]`; on the
responses wire the answer is the `response` of `response.completed`, `response.incomplete` or
`response.failed`; on the messages wire it builds each content block from its start and deltas
(a tool's `partial_json` read as its `input`), merges `message_delta`, and needs
`message_stop`. `crates/llm-gateway/tests/streamed_targets.rs` and
`crates/llm-gateway-cli/tests/runpod_host.rs` hold it: the fake proxy there sends its first event
at once and the rest over a span longer than the silence it allows, and the call completes.

A refused relay closes gracefully: the refusal is written, the write side closed, and what the
client still sends is read and discarded (up to `request-body-bytes`, within `read_timeout`), so
a reset cannot destroy the refusal before the client reads it.

### The client's own credential

DECIDED 2026-11-04 (story:subscription-passthrough). A model marked
`RelayModel::with_client_credentials` authenticates the client itself. Behind a gateway that authenticates nobody
(`IngressAuth::None`) it is sent the client's credential lines; behind one that authenticates its
owner it is sent none, since the client's `authorization` is the owner's token. The
request reaches the model as the client sent it: its request target with the query, every header
but `UNFORWARDED_HEADERS` (the hop-by-hop lines, `host`, `content-length` and `expect`) and the
ones the client's `connection` line names, `accept-encoding` included, and its body, rewritten
only where the route resolves an alias. The client's `authorization` and `x-api-key` lines
(`CLIENT_CREDENTIAL_HEADERS`) are held in a buffer overwritten when dropped. No effort mapping
(step 6) and no output clamp (step 7) apply, and a request on a wire the model does not declare is
`wire-not-served`, never translated. A model may also declare a name prefix
(`RelayModel::with_model_prefix`): every model name starting with it is served by that model and
relayed as the client sent it; the longest prefix wins, and an alias matched exactly wins over
every prefix. The binary marks a hosted entry `auth_kind = "client"`, which it admits only for
`https://api.anthropic.com`, on the messages wire, behind `ingress_auth = "none"`, so Claude Code
on a Claude subscription reaches Anthropic through a local gateway with
`ANTHROPIC_BASE_URL=http://127.0.0.1:<port>`.

The answer reaches the client the same way: its status line, every header but the hop-by-hop
ones, and its body byte for byte, re-chunked only where the target chunked it. A
content-encoded answer (`gzip`, `x-gzip`, `deflate`, `br` or `zstd`) is passed on encoded; a
copy of at most 16 MiB is decoded through the relay's `BodyDecoder`
(`Relay::with_decoder`, which the binary composes) into at most 64 MiB, and the usage is read
from the decoded copy. When the copy cannot be decoded, or is cut at either size, the call's
tokens are unknown and its record says `usage_note = "undecodable(<coding>)"`; a client that left
mid-answer is not such a case.

**Retries.** DECIDED 2026-11-04. A 429, 502, 503 or 529 answer is retried while no byte of it has
reached the client, never when the target says `x-should-retry: false`. The wait is the target's
`retry-after` (seconds or an HTTP date), else an exponential backoff of 1, 2, 4 and 8 seconds
with jitter, capped at 30 seconds. A model's attempts and its total wait are bounded by its
`retry_max_attempts` (default 4, `DEFAULT_RETRY_ATTEMPTS`) and `retry_max_wait` (default 60
seconds); when they are spent the last answer is passed through as it came. A client that sends
`x-stainless-retry-count`, as the Anthropic SDKs and Claude Code do, retries on its own, so the
gateway makes exactly one attempt for each of its requests and passes the answer through: a
retry here would multiply the client's own. The gateway retries only for a client that sends no
retry header. Every attempt is recorded in the call: its status, its wait, the
answer's relayed header lines (printable values only, within a fixed byte budget), and, for an answer outside 2xx, its
body as `error_body`: decoded when it was encoded (at most 64 KiB of it read), cut to
`MAX_ERROR_BODY` (2048) bytes, and redacted of token-shaped strings.

No record, event or dashboard field carries a credential value. The dashboard groups
subscription limits by a credential label, the first eight hex digits of the SHA-256 of the
`authorization` or `x-api-key` value, which names a login without revealing it.

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
  stops sending the format under `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS=1`, as its published
  gateway protocol says; this has not been observed against a live Claude Code client.
- **`POST /v1/messages/count_tokens`.** Not relayed, although vLLM v0.27.1 serves it
  (`vllm/entrypoints/anthropic/api_router.py:95-96`): with the owner credential, a `POST` to a
  path that is not a wire path is `method-not-allowed` (405), and no target is asked for. The
  protocol page says Claude Code falls back to estimated counts when the endpoint is missing;
  whether it treats a 405 as it treats a 404 is for the live record.
- **Headers.** `anthropic-version` and `anthropic-beta` are dropped, as every header of the
  client's request is (step 7). vLLM v0.27.1 reads neither: its messages router and serving code
  read no request header (`vllm/entrypoints/anthropic/api_router.py`,
  `vllm/entrypoints/anthropic/serving.py`). The protocol page's advice to forward them verbatim
  is written for an Anthropic upstream. A Bedrock model is sent no client header either. An
  `api = "anthropic-invoke"` model that declares `anthropic_beta = [..]` receives, as the Invoke
  body's `anthropic_beta`, the beta names of a messages client's header that its list holds, in
  the client's order. Only there, for a messages client, an `anthropic-invoke` model and a list
  that is not empty, llm 0.24.0's decoder reads the header (one line: the head refuses a repeated
  one), and a malformed one (not visible ASCII, a name outside `[A-Za-z0-9._-]` or over 128
  bytes, more than 32 names) is refused `translation-invalid` (400) before anything is sent.
  Each other name is logged as dropped by name (`model`, `field = "anthropic-beta"`, `beta`),
  because Bedrock refuses a beta it does not support and Claude Code sends several. A model with
  no list, every `converse` model, and a chat or responses client are sent none: the header is
  not read, so not refused, and its name alone is logged as dropped (`model`,
  `field = "anthropic-beta"`), never its value. The query of `POST /v1/messages?beta=true` is discarded
  like any other (step 7 asks the target on the wire's own path).
- **The answer.** vLLM's event stream, thinking blocks, `signature_delta` and `tool_use` blocks
  included, reaches Claude Code byte for byte (step 9). vLLM writes no `ping` event, and the
  gateway adds none.

## Translation

A model's declared `wires` are the wires its upstream speaks. A request on one of them takes the
byte relay above, unchanged. A request on another wire, to a relay composed with a `Translator`
(`Relay::with_translator`), is translated through llm's neutral turn for the model's **first**
declared wire. The binary always composes one (`llm_gateway_cli::LlmTranslator`, over llm 0.24.0's
ingress decoders, protocol clients and ingress encoders); a relay without one refuses the
request `wire-not-served`, as before. This is translation of a published subset, not a claim of
vendor API equivalence: a field outside the subset is refused, never dropped. The specification
is step 5 of the `Relay` command of `spec/domains/gateway.yaml`; the scenarios are
`contracts/gateway/scenarios/translate-*.yaml`.

| Client wire | Upstream `chat` | Upstream `responses` | Upstream `messages` |
| --- | --- | --- | --- |
| `chat` | relayed byte for byte | translated | translated |
| `responses` | translated | relayed byte for byte | translated |
| `messages` | translated | translated | relayed byte for byte |

### The request

The port's order, all of it before any target is asked for, so a refused request never wakes a
pod:

1. Steps 1-5 of the relay, `tools-not-served` included.
2. The body is decoded with llm 0.24.0's decoder for the client's wire. A field or value outside
   the subset, or past its size bound (llm's 16 MiB request bound included), is
   `translation-unsupported`; a value breaking one of its rules (contradictory output limits, a
   tool result answering no call, a value of the wrong kind) is `translation-invalid`.
3. Reasoning state the client replays (a Messages `thinking` or `redacted_thinking` block, a
   Responses `reasoning` item) is bound only when it is a seal this gateway made for this route
   since it last started ("Reasoning state on a translated route" below). Every other carried
   item is `translation-opaque-state`, and the request with it: it was minted by another serving
   binding, for another route or before a restart, and the gateway binds no state it cannot
   attribute.
4. The turn is checked against the model's capabilities: tools and tool choice when its
   `ToolCalling` is `Parsed`; `temperature` and `top_p`; the efforts `minimal`, `low`, `medium`,
   `high` and `xhigh`, and `none` as well when the upstream is chat or responses, each checked as
   the upstream wire receives it (llm's `wire_effort`: effort `max` reaches a chat or responses
   upstream as `xhigh` and disabled thinking as `none`, while a messages upstream receives `max`
   and `none` as they are, and so refuses them); a context window and output limit of its `max_model_len`, else its
   `context_window`, else 200000. Then it is projected for the upstream wire. Either refusal is
   `translation-unsupported` or `translation-invalid` by the same rule.

Each `translation-unsupported` or `translation-invalid` refusal of steps 2 and 4 (a Bedrock
model's check included) writes one `tracing` event at `info`, message `translation refused`, with
`model` (the alias), `ingress` (the client's wire), `refusal` (the code) and `refused`, llm's
diagnostic written escaped. llm 0.24.0's diagnostic names the refused path or field and no value
the client sent: `messages[].content[].?`, `thinking.display`, ``chat request field `n` ``, a
message role by the word "role" and never the role, `binding does not take structured output`.
An array element is written `[]`, without
its index. A key outside the subset is a key the client chose, so the line writes it `?` after
the location llm names (`?` alone at the root, `text.?`, `messages[].content[].?`), whatever its
shape: llm 0.24.0 copies such a key into its diagnostic when it is 1 to 64 bytes of
`[A-Za-z0-9_-]`, and the gateway replaces it in the three diagnostics that copy one (chat
`chat request field is outside the published subset: <path>`, responses `request body carries a
field outside the pinned subset: <path>`, messages `Messages field is outside the declared
subset: <path>`). Names from a fixed list stay named: those llm writes itself (``chat request
field `n` ``, `messages[].content[].type`) and the paths listed below as still refused by name
(`thinking.display`, `context_management`), which a key holding a secret cannot equal. The answer is unchanged: the refusal body carries the code and its fixed
message, never llm's diagnostic. `translation_a_*_refusal_names_the_refused_path_in_one_log_line`
in `crates/llm-gateway-cli/tests/translation.rs` sends each wire's refused requests with a marker
in the message text, tool names, arguments and results and in the refused value, and
`translation_a_*_refusal_writes_a_clients_key_as_a_question_mark` a secret-shaped key of 64
bytes holding it at the root and nested; neither finds it in a log line or an answer. The
first turns the translator prepares at composition to decide `GET /`'s profiles write no line:
a refused one is the answer "no profile", not a client's request
(`translation_get_root_profiles_codex_and_claude_code_where_their_turns_translate`).

```text
2026-10-30T06:21:22.891160Z  INFO llm_gateway_cli::translate: translation refused model="claude" ingress=responses refusal=translation-unsupported refused="binding does not take structured output"
```

What each client wire's decoder accepts, from llm 0.24.0. Every field a decoder reads has one rule
(llm's `docs/design.md` "Client request fields"): **carried** to every upstream wire that has the
concept, **translated** under another name, **advisory** (a hint a wire without the concept
omits), or **refused** by name. Every key a decoder does not name is refused by its path, at
every level, so a key outside the subset is `translation-unsupported`, never dropped
(`translation_guard_*` in `crates/llm-gateway-cli/tests/translation_guards.rs`); the gateway
runs no key check of its own.

- **`chat`** (`llm_chat::SUPPORTED_REQUEST_FIELDS`): `model`, `messages`, `tools`,
  `tool_choice`, `max_completion_tokens`, `max_tokens` (equal when both are given),
  `temperature`, `top_p`, `reasoning_effort`, `stream`, `stream_options.include_usage`, and
  `parallel_tool_calls`, `prompt_cache_key`, `verbosity` and `response_format`. Messages are
  `system` or `developer` before the conversation, `user`, `assistant` (text and `tool_calls`)
  and `tool`; content is a string or `text` parts, and a user message's `image_url` parts;
  tools are `function` tools with a `parameters` schema, and a `strict` function is refused;
  `tool_choice` is `auto`, `required` or a named function.
  Refused by name: `n`, `logprobs`, `top_logprobs`, `seed`, `stop`,
  `frequency_penalty`, `presence_penalty`, `logit_bias`, `functions`, `function_call`,
  `modalities`, `audio`, `prediction`, `web_search_options`, `store`, `metadata`,
  `service_tier`, `user`.
- **`responses`** (`llm_responses::ACCEPTED_BODY_FIELDS`): `model`, `input`, `instructions`,
  `tools`, `tool_choice`, `max_output_tokens`, `temperature`, `top_p`, `reasoning` (`effort`
  and `summary`), `parallel_tool_calls`, `prompt_cache_key`, `text.verbosity`, `text.format`,
  `client_metadata`, and `stream: true`, `store: false` and
  `include: ["reasoning.encrypted_content"]`, each required with exactly that value.
  `instructions` and the text parts of the `developer` messages that open `input` are the
  instructions; `input` then holds `user` and `assistant` messages of one text part each (an
  assistant part may carry an empty `annotations`; a user message may add `input_image`
  parts), `function_call` and
  `function_call_output` items (an output that is not the envelope `{"ok":…,"output":…}` is a
  successful result carrying that string), and an item may carry an `id`, which reaches no
  upstream. Tools are `function` tools with `strict: false`; a tool of another type is refused
  by its type. A translated responses request is always streamed.
- **`messages`** (`llm_messages::decode_request`): `model`, `max_tokens` (positive), `system`
  (a string or text blocks, joined by a line feed), `messages` (`user` and `assistant`, with
  string content or `text`, `image`, `tool_use` and `tool_result` blocks whose content is a
  string, and `system`: leading ones join `system`, a later one is marked user text, see below),
  `tools` (`name`, `description`, `input_schema`), `stream`, `temperature` (at most 1), `top_p`,
  `thinking` (`adaptive`, `enabled` with a budget, `disabled`), `output_config.effort` (any
  name, `max` included), `output_config.format`, `tool_choice` (`auto`, `any`, `tool`, and
  `disable_parallel_tool_use`), `metadata`, and `cache_control` (`ephemeral`, an optional
  `ttl`, one lifetime per request) on a system block, a content block or a tool.

**Structured output and images.** llm reads structured output (`response_format`,
`text.format`, `output_config.format`, since llm 0.21.0) and a user image (since llm 0.22.0) on
every client wire, but no translated path of the gateway carries either. The capabilities a
model is translated under, a Bedrock model's included, declare both `false`
(`translate::translated` in `crates/llm-gateway-cli/src/translate.rs`), whatever a routes
document declares, so llm refuses such a request `translation-unsupported` before any target is
asked for, logged `binding does not take structured output` or `binding does not take image
input` (`translation_structured_output_and_images_are_refused_on_every_translated_path` in
`crates/llm-gateway-cli/tests/translation.rs`). A model the client's wire reaches relayed is not
translated, so its upstream answers both.

How a carried or advisory field reaches each upstream wire is llm's table:

| Read as | Messages upstream | Chat upstream | Responses upstream |
| --- | --- | --- | --- |
| effort `max` | `output_config.effort` `max` (refused: not a capability) | `reasoning_effort` `xhigh` | `reasoning.effort` `xhigh` |
| thinking `adaptive` | `thinking` | nothing | nothing |
| thinking `enabled` with a budget | `thinking` | refused | refused |
| thinking `disabled`, no effort | `thinking` | `reasoning_effort` `none` | `reasoning.effort` `none` |
| `parallel_tool_calls` `b` | `tool_choice.disable_parallel_tool_use` `!b` | `parallel_tool_calls` | `parallel_tool_calls` |
| `cache_control` | on the blocks and tools the client marked, with their lifetime, and none of llm's own; a request with no mark gets llm's two breakpoints (the system block and the last message's last block) | omitted | omitted |
| `prompt_cache_key` | omitted | `prompt_cache_key` | `prompt_cache_key` |
| verbosity | omitted | `verbosity` | `text.verbosity` |
| `reasoning.summary` | omitted | omitted | `reasoning.summary` |
| an item `id`, `client_metadata`, `metadata` | omitted | omitted | omitted |

llm's client reports an omitted advisory hint once the turn has succeeded, as an
`advisory-omitted` warning event; no client wire's encoder writes a warning, so the client is
sent none.

So Claude Code's first turn (adaptive `thinking`, a `system` of two text blocks, `cache_control`
and effort `max`) and Codex's first turn (`instructions`, a developer message, item ids,
`parallel_tool_calls`, `prompt_cache_key`, `text.verbosity`, `reasoning.summary` and
`client_metadata`) are carried to another wire (scenario
`translate-a-coding-agents-turn-reaches-another-wire-with-its-controls`). Codex's second turn
replays the `reasoning` item its first answer carried: on a chat or messages route, an item this
gateway sealed for that route is bound (step 3), and the item of a recorded or foreign answer is
`translation-opaque-state`. A `system` message inside Messages `messages` is carried as llm
0.24.0 reads it: leading ones join the system prompt in order, and a later one is user text in
the mark `<system-message>\n<text>\n</system-message>`, at its place, after the tool results when
it follows a tool call. Still refused by name, `translation-unsupported`: `context_management`,
`thinking.display`, hosted and custom tools, and a `developer` message after the first item of
Responses `input`; structured output and a user image are refused by the capabilities above.

### The upstream

The target the source hands out supplies the scheme (`RelayTarget::scheme`: a hosted endpoint's
base URL scheme, `https` for a Runpod proxy endpoint), the authority, the base path and the
credential. The turn is sent once, with llm's client for the upstream wire, to
`<scheme>://<authority>` and the base path (`/v1` without one) joined to the wire's path:
`/chat/completions`, `/responses` or `/messages`. The credential goes as its kind says
(`authorization: Bearer`, the endpoint's `credential_header`, or none), resolved from the
target's `TargetBearer` (`TargetBearer::expose`); the owner's credential and the client's
headers never do, except a valid `x-opencode-session` to an OpenCode Go endpoint (a model
marked `RelayModel::with_client_credentials` is never translated). llm's client sends its own protocol headers (`accept: text/event-stream`,
`anthropic-version: 2023-06-01` on messages, and on messages to an OpenCode Go base URL, the
`x-opencode-session` of [the OpenCode Go session](#the-opencode-go-session) below) and trusts
the `webpki-roots` Mozilla roots; a
hosted endpoint's static `headers` are not sent on a translated request. The projection is
llm's: every upstream is asked for a stream (chat with `stream_options.include_usage`), a
responses upstream with `store: false`; a messages upstream gets `max_tokens` from the request,
else `min(16384, w / 2)` of the window `w` it is translated under (the model's `max_model_len`,
else its `context_window`, else the capabilities' window, 200000 by default), so the input keeps
room in the window, and never more than the capabilities' output limit,
and a messages client's `cache_control` marks where it placed them, or, when it sent none (and
for every chat or responses client), llm's two breakpoints on the system block and the last
message's last block. Neither
the messages effort mapping of step 6 nor the output clamp's count of step 7 applies.

The exchange is bounded by llm's transport: 10 s to connect, 120 s for the answer's head and
between two reads, and 3600 s in all. These are timeouts, not size bounds, so they are not in
[Bounds](#bounds); `crates/llm-gateway-cli/src/translate.rs` holds them.

#### The OpenCode Go session

OpenCode Go (a base URL starting `https://opencode.ai/inference/go/`, as llm's
`is_opencode_go` decides it for `<scheme>://<authority>` and the base path, `/v1` without one)
refuses a request without `x-opencode-session` (400 `MissingSessionID`) and routes a
conversation by it. The binary's hosted endpoint for such a base URL is sent exactly one
`x-opencode-session` on every request, relayed or translated, chosen in this order:

1. The caller's own `x-opencode-session`, when it is 1 to 256 bytes (`caller-session-bytes`)
   of printable ASCII without a space (0x21-0x7e, llm's identifier rule). Any other value is
   ignored: it is not sent, not logged and not echoed. Only SP and HTAB around it are
   dropped, as for any field. A NUL or a byte that is not UTF-8 refuses the request
   `request-malformed`, as it does in any header.
2. Otherwise `llmgw-` and the first 32 lowercase hex characters of SHA-256 over a key of 32
   random bytes the process draws once from the system random source and the caller's peer
   address (its 4 or 16 octets; an IPv4 address mapped into IPv6 counts as IPv4). One peer keeps
   one value for the process's lifetime, two peers get two, the provider never sees the
   address, and a restart derives new values. Behind a proxy every caller is the proxy's
   address and shares one session.
3. Otherwise, a request with no peer address: on a relayed request, the static
   `x-opencode-session` the endpoint's `headers` declare, if any; on a translated request,
   llm's own, minted as `llm-<process id>-<nanoseconds>-<sequence>` for each request, because
   the translator builds a client per request.

A value from 1 or 2 replaces a declared static `x-opencode-session`; the static one is a
fallback, never sent beside it. On the byte relay the session goes on every wire, the output
clamp's count included; on the translated path llm's messages client carries it
(`MessagesClient::with_session`), and a chat or responses upstream gets none, as llm sends it
there. An endpoint whose base URL is not OpenCode Go (another path or port of `opencode.ai`
included) is sent no `x-opencode-session` but a static one its `headers` declare, as before.
`crates/llm-gateway-cli/src/hosted.rs` (`HostedTarget::caller_header`, `derived_session`) holds
the order; the gateway crate holds only the `Caller` and the rule of step 1.

### A Bedrock upstream

A `[hosted.<alias>] kind = "bedrock"` model is not a target the source hands out. The binary
serves it as a model whose upstream wire is `messages` and which is translated only, so a request
on every wire, the messages wire included, takes the translated path; a relay without a
translator refuses it `wire-not-served`. The turn goes to llm 0.24.0's `llm-bedrock` client
(`BedrockClient`) for the model's `region` and `model_id`: `InvokeModelWithResponseStream` with
an Anthropic Messages body for `api = "anthropic-invoke"`, `ConverseStream` for `converse`, at
`https://bedrock-runtime.<region>.amazonaws.com` or the declared `endpoint`. No pod is woken,
nothing is acquired or held, and a failure is reported under the authority `bedrock`.

Every request is signed with AWS Signature Version 4 from llm's `ContainerCredentials`, which
fetch from `AWS_CONTAINER_CREDENTIALS_FULL_URI` presenting the token in the file
`AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` names. The binary reads that file through the
trusted-file reader at start (`container-token:<rule>`) and again on every fetch. Unlike every
other trusted file, its path is resolved through symbolic links first and the rules are checked
on the file it resolves to: a Kubernetes projected volume, as EKS Pod Identity mounts the token,
links `<name>` to `..data/<name>` and `..data` to a timestamped directory the kubelet swaps on
rotation (layout from the AWS and Kubernetes documentation, unverified here against a cluster).
The resolved file must lie inside the directory the token path names; a link out of it is
refused `container-token:symlink`, naming neither the token path nor the file it resolves to.
Resolving on every read means a rotated token is used without a restart. A refusal at start
names the variable, `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE: <detail>`, never the path. The read
runs on the runtime's blocking pool, and a file a fetch refuses makes that turn `unauthorized`
with a `warn` line naming the rule (`rule`, such as `container-token:unsafe-mode`) and its detail
(`detail`, such as `has mode 666; none of 022 may be set`), never the path or the token. The
credentials client gives up connecting after 5 s, reading
after 10 s and in all after 30 s. The variables are read only when the document has a Bedrock
model (`container-credentials:unset`, `container-credentials:uri`).

Preparation differs from an endpoint's in four ways. A reasoning effort, from any client wire,
reaches an `anthropic-invoke` model as llm's Messages encoder writes it, `output_config.effort`
in the Invoke body, when the model's `reasoning_efforts` holds it: a Bedrock-only list of
`low`, `medium`, `high`, `xhigh` and `max`, each at most once, all five when undeclared, `[]`
for a model that takes no effort (`config:value` on an endpoint, on a `converse` model or for
another name). An effort the list does not hold is refused `translation-unsupported` before
anything is sent, never dropped. A field a `converse` model does not take is dropped, not
refused, and its name alone is logged at `info` (`model`, `field`; never a value, a prompt or
an answer): a reasoning effort and parallel tool calls, because llm's Converse encoder refuses
both and Converse has no field of its own for either (an Anthropic model's effort could travel
in `additionalModelRequestFields`, which llm does not write), and thinking on a model id that
is not Anthropic's, which only Anthropic models take there. A request without an output limit is sent
`min(16384, w / 2, m)` of its window `w` (200000 by default) and its `max_output_tokens` `m`
(1 to `w`, `w` when undeclared, a Bedrock-only key the inventory lists); a request asking for
more than `m` is refused as for any limit its capabilities set. And reasoning state a client sends
back on the messages wire to an `anthropic-invoke` model is passed as that model's own; any other
carried state is bound or refused as for an endpoint. A messages client's `cache_control` marks
reach an `anthropic-invoke` model on the blocks and tools it marked, with their lifetime, and no
breakpoint of llm's own is added; a request without a mark, and every chat or responses client's,
gets llm's two breakpoints, the system block and the last message's last block
(`bedrock_a_clients_only_mark_on_its_system_block_is_the_only_mark_invoke_receives`,
`bedrock_a_clients_marks_on_a_block_and_a_tool_reach_invoke_in_place_with_their_ttl`,
`bedrock_a_request_without_a_mark_keeps_the_automatic_breakpoints` in
`crates/llm-gateway-cli/tests/bedrock.rs`). Every turn is then checked with llm
0.24.0's `llm_bedrock::check_request` for the model's target, the same check its client runs
before it encodes, so a turn llm's Bedrock encoder would refuse is refused before anything is
sent, `translation-unsupported` for what Bedrock cannot carry. For a `converse` model that is,
among llm's rules, a conversation that does not open with a user message or a tool result, tool
calls or tool results without `tools`, a tool name or call id that is not 1 to 64 of
`[A-Za-z0-9_-]`, tool arguments or a tool schema that is not an object, and carried state that is
not a Converse reasoning block; the gateway keeps no copy of these rules. An `anthropic-invoke`
model that declares `anthropic_beta = [..]` (at most 32 distinct names by llm's beta name rule;
`config:value` on an endpoint or a `converse` model) is sent, in the Invoke body's
`anthropic_beta`, the names of a messages client's `anthropic-beta` header that the list holds
("Claude Code on the messages wire", Headers, has the rules). The answer is written as on any translated
route; a messages client of an `anthropic-invoke` model also receives its thinking blocks, which
it sends back on the next turn.

### An Ollama upstream

DECIDED 2026-11-25 (story:ollama-route). A `[hosted.<alias>] kind = "ollama"` model is a hosted
endpoint at an Ollama server's root (`http://127.0.0.1:11434` unless `base_url` names another),
sent no credential, served on the wires it declares (all three unless `wires` names fewer), the
alias replaced by `upstream_model`, the Ollama model name. Its table takes no credential key and
no `model_prefix`, each refused at start as `config:value` naming the key and quoting no value:
the model's status and its `model-not-pulled` refusal are read for `upstream_model` alone, and a
prefix would send other names that status does not describe
(`adv_ollama_a_prefix_for_a_name_the_server_lists_is_refused_at_start` in
`crates/llm-gateway-cli/tests/wake_status.rs`). It is kind `ollama` in
[the status list](#the-status-list), and the route inventory lists its targets under provider
`ollama`, account `local`. Ollama serves all three wires itself, so each declared wire is relayed
byte for byte to Ollama's own path: chat to `POST /v1/chat/completions`, responses to
`POST /v1/responses` and messages to `POST /v1/messages` (checked on Ollama 0.40.18). A wire the
model does not declare is served as a hosted endpoint's is: translated onto its first declared
wire, the translator's request going to that wire's path. The tokens the server reports reach the
usage record as a hosted endpoint's do.

The gateway sends an Ollama server only `GET /api/tags` and `GET /api/ps` (the status read of
[the lifecycle routes](#the-lifecycle-routes)), `POST` to the three wire paths, and
`POST /v1/messages/count_tokens`: the output clamp's count (relay step 7) when the model
declares `context_window` and a messages call sets `max_tokens`, and a client's own count call.
The output clamp's count is sent under the Ollama model name and its `input_tokens` cuts
`max_tokens` as on any target
(`ollama_the_output_clamp_counts_on_the_server_under_the_ollama_model_name`), against a fake
server only: that path was not checked against a running Ollama, and a count it does not answer
with `input_tokens` leaves `max_tokens` as sent. It sends no request that changes what the
server stores: no `/api/pull`, `/api/create`, `/api/copy`, `/api/delete` or `/api/push`
(`ollama_the_binary_sends_the_server_only_its_lists_and_the_wire_calls` and
`ollama_the_gateway_never_asks_the_server_to_change_what_it_stores` in
`crates/llm-gateway-cli/tests/wake_status.rs`). The operator pulls a model with Ollama's own
command line.

Each call's acquisition reads the model's status first, the same bounded read the status route
sends, reused for `status-reuse-seconds`. A model the server answered its tags without is
refused `model-not-pulled` (503, no `retry-after`, outcome `refused`), its message naming the
alias. A model that is pulled and not loaded is relayed: the call loads it, and waits for the
load under the bounds every hosted connection has, 10 s to connect (`ENDPOINT_CONNECT_TIMEOUT`)
and 120 s for each read and write (`ENDPOINT_IO_TIMEOUT`), so a load that writes no answer byte
for 120 s ends `upstream-failed`; a translated call waits under llm's transport, 120 s for the
answer's head. No bound of its own is added. A server that does not answer is no refusal: the
call goes on and fails as any hosted endpoint's call does.

### The answer

The answer is written on the client's wire, its head once, on the first event or at the
outcome: `200` with `content-type: text/event-stream` and chunked framing for a streamed request
(responses always; chat and messages with `stream: true`), and `application/json` with a
`content-length` for a non-streamed one (`chat.completion` or a `message` object), each with
`cache-control: no-store` and `connection: close`. Each server-sent event is the one llm's
encoder writes: `event: <type>` and `data: <json>` on responses and messages, `data: <json>` on
chat, which ends `data: [DONE]`. Identifiers (`chatcmpl-`, `resp_`, `msg_`) and `created` are
the gateway's own. A messages answer names the alias the client asked for as its `model`, in
`message_start` and in the non-streamed `message`, never the model the target reported or the
name it was sent; an alias that is not a Messages model name is named nowhere. The upstream's reported counters are carried and an unreported one stays
unreported; on a messages client the input total is sent only when the upstream reports both
cache counters, which only a messages upstream does. Reasoning text is relayed where the
client's wire has a place for it (chat `reasoning_content`, a responses reasoning summary) and
withheld on a messages client, whose `thinking` block needs a signature only a messages
upstream makes. Continuation state the upstream mints for its own wire (Messages signed
thinking) reaches a responses client sealed for the route, as the next section says, and no
other client.

#### Reasoning state on a translated route

Codex replays the reasoning items of its previous answer on every turn. So that a translated
route serves that turn without binding state it cannot attribute, the gateway hands a responses
client reasoning state it sealed itself, and binds only that.

- **The key.** The translator generates one HMAC-SHA256 key from the system's random source when
  it starts. It is never written out, logged or rendered. A gateway that could not generate one
  seals nothing and answers as if the upstream minted no state.
- **What a responses client is handed.** Each signed `thinking` or `redacted_thinking` block of
  a messages upstream's answer reaches it, in its place, as a finished `reasoning` item whose
  `encrypted_content` is the seal: `llmgw-seal-1.`, the block as JSON in unpadded base64url, `.`,
  and in unpadded base64url the HMAC of a fixed label, the route (the model's alias and its
  upstream model), the client's wire, the upstream wire and those JSON bytes, each
  length-prefixed. The first such item is the one the streamed reasoning opened, under its `id`,
  with the reasoning shown as its summary. A chat upstream mints no state; when it shows
  reasoning (`reasoning_content`), that item is finished with a seal over `null`. The seal does
  not hide the block, whose thinking the summary already shows; it attributes it to this route.
- **What is bound.** A replayed `reasoning` item from a responses client whose
  `encrypted_content` verifies, in constant time, as a seal of the running key for the request's
  route and wires: the block it carries is sent to the messages upstream at the item's place, as
  the upstream gave it, under the binding of the target the relay acquired; a seal over `null` is
  dropped before the turn is projected for the chat upstream. The item's `id` and `summary` are
  not read.
- **What is refused**, `translation-opaque-state` (400) for the whole request before any target
  is asked for: an item without a seal (a recorded turn, or state another gateway or the
  upstream itself minted); a seal any character of which was altered, or one forged; a seal made
  for another route, or for the same alias on another pair of wires; and a seal made before the
  gateway last started, whose key is gone. A client that keeps its conversation across a restart
  of the gateway starts a new one.
- **What is not sealed.** Nothing toward a chat client (its wire carries no opaque state) or a
  messages client (llm's messages encoder places a block the stream did not show after every
  streamed block, so a replayed block would reach the upstream out of its place); their answers
  carry no upstream state, and a `thinking` block such a client replays is
  `translation-opaque-state`. A same-wire route is relayed byte for byte, and its reasoning state
  with it, unchanged.
- **What is never logged.** No log line, event, metric, usage record or refusal carries a seal,
  the key or the state a seal holds.

Scenarios `translate-sealed-reasoning-state-is-bound-on-the-route-that-sealed-it`,
`translate-sealed-reasoning-state-from-another-route-is-refused`,
`translate-altered-or-forged-sealed-reasoning-state-is-refused` and
`translate-sealed-reasoning-state-from-before-a-restart-is-refused` observe each case;
`crates/llm-gateway-cli/tests/translation.rs` runs Codex's recorded two-turn exchange through a
responses-to-chat and a responses-to-messages route, and `seal_*` in
`crates/llm-gateway-cli/src/seal.rs` check every character of a seal and every part of its route.

A failure before any output, by llm's class of it and whatever was dispatched:

| llm class | Upstream | Answer | Target |
| --- | --- | --- | --- |
| `transport`, `deadline`, `unavailable` | unreachable, a `408` or `5xx`, timed out (no head in 120 s, or 120 s between reads after it), a connection reset or ended before its declared length or inside a chunk | `upstream-failed` (502) | reported to the target source, as at step 8 |
| `unauthorized`, `rate-limited`, `refused`, `invalid-request`, `unsupported` | an answer's status refused the request (`401`, `403`, `429`, another `4xx`) | `upstream-refused` (502) | kept |
| `protocol` | an answer that broke its wire: a stream ended cleanly before its terminal event, a body that is no event stream, a call to a tool the request did not offer | `upstream-refused` (502) | kept |
| `too-large`, after dispatch | the answer passed a [bound of the translated path](#bounds-of-the-translated-path) | `translation-too-large` (502) | kept |

Unlike the byte relay, which relays a `500` as the model's own answer, a translated request
cannot carry an upstream's error body to another wire, so it is never relayed. An answer llm's
encoder refuses before any output is `translation-unrepresentable` (502). After output, an
upstream failure, a bound or an encoder refusal ends the stream with the wire's terminal error
and nothing else: `response.failed` (responses), an `error` event (messages), or
`data: {"error":{"code":"translation-unrepresentable"|"translation-too-large"|"upstream-failed",…}}`
and no `[DONE]` (chat); a bound's terminal error carries `translation-too-large`'s message,
which names the published bounds. A client that stops reading (a write to it fails) cancels the turn,
which drops the upstream connection; so does a client waiting for a non-streamed answer that
has closed its connection or its sending side, which the gateway asks between two events
(`TranslatedAnswer::gone`, the byte relay's read that does not wait, which discards bytes sent
after the request). Nothing is retried or replayed. The target is released after the last byte. The usage record carries the status of the answer
written and its translated bytes. A translation that wrote no answer, refused or failed after
its target answered, carries the status the target answered with, which the translator tells
the relay (`TranslatedAnswer::target_status`; the binary reads it from llm's response tap).

### Known limits

Each statement about a carried or refused field is backed by a test in
`crates/llm-gateway-cli/tests/translation.rs` that prepares the request through llm.

- **Claude Code.** llm 0.24.0 carries its first turn to a chat or a responses upstream:
  `thinking` adaptive, a `system` of two blocks, `cache_control` on a block, and effort `max`,
  which reaches either wire as `xhigh`
  (`translation_claude_codes_first_turn_is_carried_to_chat_and_responses`). It needs a model
  whose capabilities name `xhigh` and allow its `max_tokens` 32000, which a routes-document
  route's default capabilities do not. Still refused: structured output
  (`output_config.format`,
  `translation_structured_output_and_images_are_refused_on_every_translated_path`),
  `thinking.display`, `context_management`, `safeguards` and a `tool`
  role (`translation_fields_llm_still_refuses_are_refused_by_name`), and an effort `max` or
  `none` toward a messages upstream
  (`translation_an_effort_is_checked_as_the_upstream_wire_receives_it`). `GET /` profiles Claude
  Code for a translated model only when its recorded first turn passes
  (`translation_codex_and_claude_code_are_admitted_where_llm_carries_their_first_turns`,
  `translation_a_client_is_not_admitted_where_llm_refuses_its_first_turn`), and that profile
  exports `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'`, which Claude Code's published gateway
  protocol says stops the structured output a translated path refuses. That has not been
  observed against a live Claude Code client, and what the Claude Code feature that sends it
  does without it is not published.
- **Codex.** llm 0.24.0 carries its first turn to a chat and a messages upstream
  (`translation_codexs_first_turn_is_carried_to_chat`,
  `translation_codexs_first_turn_is_carried_to_messages`), and a later turn replaying the
  reasoning item the gateway sealed for the route. Still refused: `text.format`
  (`translation_structured_output_and_images_are_refused_on_every_translated_path`) and a hosted
  tool (`translation_fields_llm_still_refuses_are_refused_by_name`), and a replayed reasoning item the
  gateway did not seal for the route, `translation-opaque-state`
  (`translation_a_replayed_item_the_gateway_did_not_seal_for_this_route_is_refused`). `GET /`
  profiles Codex for a translated model only when its recorded first turn passes.
- **Admission is one turn.** `GET /` admits a client by its recorded first turn only: a later
  turn, or a field a newer client sends, can still be refused by name.
- **Encoder refusals** (llm `story:ingress-block-boundaries`): a responses client cannot be sent
  two assistant messages back to back, or a message another item interrupts (text, reasoning,
  text from a chat upstream; text, a call, text); a messages client cannot be sent redacted
  thinking between streamed blocks, or tool arguments that are not a JSON object; a chat client
  cannot be sent a terminal reason it has no name for (an `incomplete` other than its content
  filter). Before any output (a non-streamed answer, or a stream that had written nothing)
  each is the 502 JSON refusal `translation-unrepresentable`; after output it ends the stream
  with the terminal error above.
- **Long answers.** An answer past 65536 events (about that many output tokens from a vLLM
  upstream), 32 MiB, a 1 MiB line or event, or a tool call of more than 64 KiB of arguments
  cannot be translated whole; see
  [Bounds of the translated path](#bounds-of-the-translated-path).
- **An Ollama model deleted inside the reuse window.** A call's acquisition reuses the model's
  last status read for `status-reuse-seconds` ([an Ollama upstream](#an-ollama-upstream)), so a
  model deleted from its server inside that window is still relayed, and the client gets
  Ollama's own 404 (`{"error":{"message":"model '<name>' not found",...}}` on chat and
  responses, a `not_found_error` on messages) rather than `model-not-pulled`. The next read after
  the window refuses it (`ollama_acquire_refuses_only_a_model_the_server_answered_without` in
  `crates/llm-gateway-cli/tests/wake_status.rs`).

## Counters and usage records

Every request to a wire path that passed owner authentication makes one `UsageRecord` once its
answer or refusal is written, before the connection is closed: the alias of the relayed model the
body named (absent when it named none), the wire, its `Disposition` (`Relayed`, `Refused` or
`UpstreamFailed`), the refusal code when it was not relayed, the status the client received, the
status the target answered with (absent when no target answered), the decoded body bytes of a relayed answer (0 for a refusal) and the milliseconds from the end of the
request head to the last byte written, and its `Outcome` ([call outcomes](#call-outcomes)). The input, output, cache read and cache creation token counters come from the usage the answer
carried (decoded when it is content-encoded) and stay absent when it reported none; the model the
target reported and the reasoning counter are always absent for now. `cost_usd` is what the call
cost when its model declares a token price ([the cost of a call](#the-cost-of-a-call)). A request, relayed or translated, whose client went away before any
answer was written (a non-streamed client that closed while it waited) is recorded `Relayed`
with status 0,
no target status and 0 response bytes: status 0 means no answer was written because the client
left. An unauthenticated or shed request makes no record. Each record carries the call's id
(`UsageRecord::call`, [call ids and call records](#call-ids-and-call-records)). The record feeds
the counters, then leaves the crate through the `UsageRecords` port the embedding implements
(`Relay::with_records`); the binary writes each as one `tracing` event at `info`, target
`usage`, whose fields are `call`, the record's values and the token counters it carries under
the call record's names (`input_tokens`, `output_tokens`, `cache_read_tokens`,
`cache_creation_tokens`); a counter the record does not carry is no field.

The counters are read from the answer's usage object only, never from a member of the same name
elsewhere in the body (a tool call's arguments, a text block, a metadata object). The usage
object is the member `usage` of a top-level JSON object, or of its `message` or `response`:
on the messages wire the message's `usage`, on a stream `message_start`'s `message.usage` and
`message_delta`'s `usage`; on chat the `usage` of the answer or of the stream's last chunk, with
`cached_tokens` and `cache_creation_input_tokens` in its `prompt_tokens_details`; on responses
the response's `usage`, on a stream `response.usage`, with the same two in its
`input_tokens_details`. A counter is a whole number; each keeps the largest value the answer
reported (`message_delta` repeats the output count, cumulative).

A streamed chat answer reports its usage only when the client asked for it: a chat client sends
`"stream_options":{"include_usage":true}` in its request body, and the target then ends the stream
with one event that carries `usage`. The gateway relays the body as the client sent it and adds
nothing, so a streamed chat call whose client did not ask has no token counters, in its record
and in its call record. A non-streamed chat answer, a responses answer and a messages answer carry
their usage without being asked.

`GET /metrics` serves llmgw's 13 counters under their llmgw names, so a scrape configured for
llmgw keeps working, and four of the gateway's own after llmgw's eight process-wide series
(`llmgw_pod_start_spend_refusals_total`, `llmgw_interaction_records_dropped_total`,
`llmgw_interaction_records_skipped_total`, `llmgw_interaction_records_failed_total`), and after the route series two per model: the outcome counter
and the latency histogram ([latency and outcomes per model](#latency-and-outcomes-per-model)). Its
prefix keeps one scrape configuration for every series. It takes the owner credential and readiness like inspection, and answers
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
| `llmgw_pod_start_spend_refusals_total` | | pod starts the embedding refused for a spend ceiling, each answered `spend-ceiling` (`Metrics::count_pod_start_spend_refusal`) |
| `llmgw_interaction_records_dropped_total` | | finished calls the embedding could not queue for its interaction record (`Metrics::count_interaction_record_dropped`, [finished calls](#finished-calls)) |
| `llmgw_interaction_records_skipped_total` | | finished calls whose record the embedding left out: bytes over the bound (`CallBytes::OverBound`) or not decoded (`CallBytes::Undecodable`), or a call that lifts to no step (`Metrics::count_interaction_record_skipped`) |
| `llmgw_interaction_records_failed_total` | | finished calls the embedding queued for its interaction record and then failed to store (`Metrics::count_interaction_record_failed`) |
| `llmgw_route_requests_total` | `model`, `wire` | records naming a relayed model on a wire it declares |
| `llmgw_route_refusals_total` | `model`, `wire` | the same, refused or no target answered |
| `llmgw_route_upstream_status_failures_total` | `model`, `wire` | the same, the target answered outside 2xx: a 4xx or 500 relayed, or a 502, 503 or 504 answered `upstream-failed` |
| `llmgw_route_cold_start_holds_total` | `model`, `wire` | requests held for a starting target: the target says it held them (`RelayTarget::held`), or `acquire` answered `model-cold-start` |
| `llmgw_route_response_bytes_total` | `model`, `wire` | the records' response bytes |
| `llmgw_requests_total` | `model`, `outcome` | records naming a relayed model, by how the call ended |
| `llmgw_request_duration_seconds` | `model` | a histogram of the same records' durations, in seconds |

Every (model, wire) pair a relayed model declares is registered at zero when the relay is
composed, so an uncalled route reads 0 instead of being absent; a request for another pair, a
translated request on a wire its model does not declare included, feeds the process-wide series
only. A gateway composed without a relay serves the process-wide series alone. An embedding
that counts pod events shares one `Metrics` with the relay
(`Relay::with_metrics`). The upstream and status series count as llmgw does (llmgw
`src/lib.rs:598-629` at `048ebd8`). The specification is `spec/domains/telemetry.yaml`.

### The cost of a call

A model may declare what its tokens cost, in USD per 1000000 tokens: a Runpod model as
`[models.<alias>.token_price]`, a hosted model of every kind as `[hosted.<alias>.token_price]`
(keys `input` and `output`, required, `cache_read` and `cache_creation`, each a quoted decimal
string), and a route of the routes document as its `price`, read as llm writes it (`currency`
`USD`; `input`, `cached_input`, `cache_creation_input` and `output` become `input`,
`cache_read`, `cache_creation` and `output`). A value llm's `Amount::parse` refuses, a negative
one, a missing `input` or `output`, an unknown key and a route priced in another currency are
refused `config:value` naming the key and quoting no value. `price_per_hour_usd` is a pod's price
per hour and has nothing to do with it.

The gateway does no arithmetic on money. The embedding gives a priced model a `CallPricing`
(`RelayModel::with_pricing`); the binary's is the declared price as the one model of an llm
`PriceBook` (crate `llm-cost`), and `PriceBook::quote` prices the call: each class's count at
its rate, each line rounded up once to a nanodollar, the lines summed. The record's `cost_usd`
is that total as llm writes an `Amount` (`0.00756`, `3`).

The counters are read as llm counts them. On the messages wire `input_tokens` excludes the cache
reads and writes; on chat and responses the prompt tokens include them, and the uncached input is
what remains. A cache class the answer did not report counts no tokens.

`cost_usd` is present only when all of these hold, and absent otherwise, never `0` and never an
estimate:

- the call was relayed and its answer ended as written: no refusal, no `upstream-failed`, no
  terminal error after the head, no client that went away, no answer its upstream cut short (a
  read of the answer's body that failed after its head was written, relay step 9); an answer
  cut short keeps its outcome, `ok` on the byte relay and on a client-credential route, as
  [call outcomes](#call-outcomes) names no other for it, and only its cost is left out;
- its model declares a price;
- the answer reported its input and its output tokens;
- every class with a count above 0 has a rate: cache reads reported against a price without
  `cache_read` leave the call without a cost, while a count of 0 in a class without a rate costs
  nothing;
- the counters agree with each other (no more cache tokens than prompt tokens).

A terminal error after the head leaves the call without a cost on every relay path: the byte
relay, a client-credential route and a translated answer. The gateway reads it from the answer's
bytes as they pass, as it reads the usage, whether a translator told it
(`TranslatedAnswer::failed`) or not: a top-level object whose `type` is `error` (the messages
wire's `error` event, the responses wire's `error` event) or `response.failed` (responses), or
whose `error` is an object (the chat wire's error chunk, `data: {"error":{…}}`). An `error` of
`null`, as a completed response carries, and a `type` or `error` below the top level, such as
inside a tool call's arguments, is none. Such a call's outcome is what [call
outcomes](#call-outcomes) gives it: `upstream_error` for a translated answer that told it,
`ok` for a relayed one.

The binary writes it as the `cost_usd` field of its `usage` event, and the record `GET /v1/calls/{id}`
answers carries it as its key `cost_usd`, after the token keys ([call ids and call
records](#call-ids-and-call-records)).

### Latency and outcomes per model

Every relayed model, Runpod or hosted or a route of the routes document, is registered at zero
in both per-model series when the relay is composed: all four outcomes, every bucket, the sum and
the count. Each usage record naming a relayed model feeds both, whatever wire it came on; a
record naming no relayed model feeds neither. The `model` label is the alias from the deployment
document and `outcome` a fixed word: no label or value carries a credential, a prompt, a
completion, an upstream model name or an endpoint address.

`llmgw_request_duration_seconds` observes the record's `duration_ms`, from the end of the request
head to the last byte written, in seconds. A record falls in every bucket whose bound is at least
its duration, so the buckets are cumulative, as the Prometheus text format writes them, followed
by `_sum` in seconds and `_count`.
The histogram's buckets are `0.05`, `0.1`, `0.25`, `0.5`, `1`, `2.5`, `5`, `10`, `30`, `60`, `120`, `300` and `+Inf` seconds.

```text
llmgw_requests_total{model="code",outcome="ok"} 3
llmgw_requests_total{model="code",outcome="upstream_error"} 1
llmgw_requests_total{model="code",outcome="timeout"} 0
llmgw_requests_total{model="code",outcome="refused"} 1
llmgw_request_duration_seconds_bucket{model="code",le="0.05"} 5
…
llmgw_request_duration_seconds_bucket{model="code",le="+Inf"} 5
llmgw_request_duration_seconds_sum{model="code"} 0.012
llmgw_request_duration_seconds_count{model="code"} 5
```

### Call outcomes

A relayed answer is `ok`, whatever its status: a 4xx or a 500 the target answered reached the
client. A refusal is counted by its code. A translated answer whose head was written and which
the translator ended with its wire's terminal error (an upstream failure after output, an answer
past a bound, or one the client's wire cannot carry) is counted by that code, `upstream_error`,
though its client was answered `200`: its record stays `Relayed` with that status and no refusal
(`TranslatedAnswer::failed`). `upstream-failed` is `timeout` when the target's read
timeout passed before its answer head arrived, and `upstream_error` otherwise (an unreachable
target, a connection closed without an answer, a malformed head, or a 502, 503 or 504 from the
target). A translated request's transport timeout is reported by the translator as an upstream
failure, and is `upstream_error`. `crates/llm-gateway/src/metrics.rs` checks this table against
every code the gateway can emit.

| Code | Outcome |
| --- | --- |
| `body-incomplete` | `refused` |
| `body-not-allowed` | `refused` |
| `body-not-json` | `refused` |
| `body-too-large` | `refused` |
| `call-unknown` | `refused` |
| `context-exceeded` | `refused` |
| `credential-absent` | `refused` |
| `credential-malformed` | `refused` |
| `credential-rejected` | `refused` |
| `credential-review-unavailable` | `refused` |
| `method-not-allowed` | `refused` |
| `model-absent` | `refused` |
| `model-cold-start` | `timeout` |
| `model-not-pulled` | `refused` |
| `model-stopped` | `refused` |
| `model-unknown` | `refused` |
| `overloaded` | `refused` |
| `path-unknown` | `refused` |
| `request-malformed` | `refused` |
| `request-too-large` | `refused` |
| `route-unknown` | `refused` |
| `spend-ceiling` | `refused` |
| `target-unavailable` | `refused` |
| `tools-not-served` | `refused` |
| `server-tool-not-served` | `refused` |
| `translation-invalid` | `refused` |
| `translation-opaque-state` | `refused` |
| `translation-too-large` | `upstream_error` |
| `translation-unrepresentable` | `upstream_error` |
| `translation-unsupported` | `refused` |
| `unavailable` | `refused` |
| `upstream-failed` | `upstream_error`, or `timeout` when the target's read timed out before its answer head |
| `upstream-refused` | `upstream_error` |
| `wire-not-served` | `refused` |

A code that makes no usage record (a credential refusal, `overloaded`, a request before the
head was read) is in the table for completeness and is never counted.

### Call ids and call records

Every request to a wire path that makes a usage record (it passed authentication or its review,
or needed none under `IngressAuth::None`) is given an id when its head is decided, before its
body is read, and every answer the gateway writes to it carries the id after the status line:

```text
x-llm-gateway-call: 3f9c0e5a7d1b42c8a6e4f0b9d2c71a85-1c
```

A relayed answer, a translated one, a passthrough answer of a model that takes the client's own
credential, and a refusal each carry it; a target's own line of that name is not passed on, so the
client reads one. A request that is shed (`overloaded`) or fails authentication or its review, and
every request to another path, gets none. An id is 1 to 64 characters of `A-Z a-z 0-9 -`. The
binary's are 32 lowercase hexadecimal digits drawn once per process from the operating system's
random source, a `-`, and a counter in lowercase hexadecimal that starts at 1, so no id repeats
within a process or after a restart. An id is no secret and grants nothing.

`GET /v1/calls/{id}`, and `HEAD` without the body, answers `200` with that call's record as one
JSON object. The keys come in this order, and a value the usage record does not carry is an
absent key, never 0 and never an estimate:

| Key | Value |
| --- | --- |
| `format` | `llm-gateway.call/1` |
| `id` | the id |
| `state` | `in-flight` until the last byte or the refusal is written, then `ended` |
| `model` | the relayed model's alias the body named |
| `wire` | `chat`, `responses` or `messages` |
| `disposition` | `relayed`, `refused` or `upstream-failed` |
| `outcome` | `ok`, `upstream_error`, `timeout` or `refused` ([call outcomes](#call-outcomes)) |
| `refusal` | the refusal code |
| `status` | the status the client received |
| `target_status` | the status the target answered with |
| `input_tokens` | the input tokens the answer reported |
| `output_tokens` | the output tokens it reported |
| `cache_read_tokens` | the cache read tokens it reported (`UsageRecord::cached_input_tokens`) |
| `cache_creation_tokens` | the cache creation tokens it reported (`UsageRecord::cache_creation_input_tokens`) |
| `cost_usd` | what the call cost in USD, a JSON string as llm writes an amount (`"0.0001299"`), when its model declares a token price ([the cost of a call](#the-cost-of-a-call)) |
| `response_bytes` | the decoded body bytes relayed |
| `duration_ms` | from the end of the request head to the last byte or the refusal written |
| `profile` | the profile of the pod that served the call |
| `principal` | the service account a reviewed bearer named |

An `in-flight` record carries `id`, `state`, `wire`, `principal`, and `model` once the body named
a relayed model; an `ended` one carries the usage record's values:

```json
{"format":"llm-gateway.call/1","id":"3f9c0e5a7d1b42c8a6e4f0b9d2c71a85-1c","state":"ended","model":"code","wire":"chat","disposition":"relayed","outcome":"ok","status":200,"target_status":200,"input_tokens":11,"output_tokens":7,"cache_read_tokens":3,"response_bytes":131,"duration_ms":42}
```

The route takes the credential `GET /v1/launch` takes: the owner's bearer whatever the
`IngressAuth` (a reviewed service account's is `credential-rejected`), and none on a gateway open
on loopback. It answers with or without a relay, before `mark_ready` and while draining. Another
method is `method-not-allowed` with `allow: GET, HEAD`, and a declared body `body-not-allowed`. An
id the gateway does not hold is `404 call-unknown`, whether it is of another form, was never given
out, was given out by an earlier process, or belonged to an ended call no longer held; the answer
does not say which. The gateway holds every call in flight and the last `MAX_ENDED_CALLS` (1024,
`ended-calls-held` in [bounds](#bounds)) ended ones, and drops the oldest ended one first. The store
is in memory: a restart holds none. Reading a record asks no target, makes no usage record and
counts no series, and the store is never locked while an answer is written or a target read.

### Finished calls

DECIDED 2026-11-26 (story:finished-call-facts). Every call that is given an id
([call ids and call records](#call-ids-and-call-records)) hands one `FinishedCall` to the
`FinishedCalls` port the embedding composes with `Relay::with_finished_calls`, once, where its
usage record is made: relayed, translated, passthrough and refused calls alike. A token count
(`POST /v1/messages/count_tokens`), a shed request and one that fails authentication or its review
hand none. Without a port the value is made and dropped. The dashboard reads its finished call from
the same value.

| Field | Value |
| --- | --- |
| `call_id` | the usage record's `call` |
| `session_hash` | the session its headers name, as the dashboard keys a call: 12 hexadecimal digits of SHA-256 over `llm-gateway.session/1`, the header's source and its value, never the value itself. A call refused before its headers were read into the dashboard (a method, framing or readiness refusal) still carries it; the dashboard shows that call with no session |
| `harness` | `ClaudeCode`, `Codex` or `Other`, as the dashboard labels the sender |
| `path` | the wire's path the client called |
| `wire` | the inbound wire; a translated call's upstream speaks another |
| `route` | the alias the body named (the record's `model`) |
| `upstream` | the authority of the target the call was handed |
| `attempts` | each attempt of a passthrough call, 1 on the byte relay and the translated path once a target was handed out, 0 when none was |
| `queue_wait_ms`, `ttft_ms` | the dashboard's queue wait and time to first byte |
| `declared_cost_usd` | the record's `cost_usd`, present exactly when it is |
| `ending` | `Refused(code)`; else `UpstreamCut` when a translator told `upstream-failed` after the answer's head; else `WireError` when the answer carried its wire's terminal error, whether or not its upstream cut it afterwards; else `UpstreamCut` when the upstream cut the answer; else `ClientDisconnected`; else `Complete` |
| `started_ms`, `finished_ms` | milliseconds since the Unix epoch; their difference is the record's `duration_ms` |

The bytes of a call are copied only when the relay declares a bound with
`Relay::with_call_bytes(bound)`; without it the port receives `None` and no byte is copied. With
it, a call whose request body and answer body together are at most `bound` bytes arrives with
`CallBytes::Copied`: the request body as the gateway read it, and the answer as the client
received it, de-chunked (`AnswerBytes::Body`), or for a `text/event-stream` answer the `data:`
payload of each whole event in order (`AnswerBytes::Events`). A translated call's answer is the
translated one the client received; a refusal has its request and no answer. A call over the
bound arrives with `CallBytes::OverBound` and both sizes, its copy dropped the moment it passed
the bound. The client's answer is the same byte for byte with the copy on or off.

An answer with a content coding (a passthrough answer the client asked compressed) is copied as
the client reads it, decoded by the relay's `BodyDecoder` (`Relay::with_decoder`; the binary's
decodes gzip, deflate, br and zstd), and the bound counts its decoded bytes. One the decoder does
not decode whole (no decoder, a coding it does not know, a decode error, a client that left
before its end, more than 16 MiB as received or 64 MiB decoded) arrives with
`CallBytes::Undecodable`: no bytes, the request's size, the answer's size as received and its
`content-encoding`.

The value maps onto sessionspec's `GatewayFacts` and `WireCall` by name, with three conversions
the embedding makes: `attempts` to an `i64`, `declared_cost_usd` (a decimal string) to
`declared_cost_microdollars`, and `ending` to `incomplete` (`Complete` is none). `wire` and the
copy fill `WireCall`'s `wire`, `request` and `answer`. An embedding that cannot queue a value
counts `Metrics::count_interaction_record_dropped`, one that leaves out a call over the bound,
not decoded or lifted to no step counts `Metrics::count_interaction_record_skipped`, and one
that queued a value it then failed to store counts `Metrics::count_interaction_record_failed`.
The binary is one such
embedding: with `[interaction_records]` in its document it lifts each value with sessionspec's
`wire::lift_call` and appends the steps to a SQLite file on one writer thread
(`spec/domains/deployment.yaml` `InteractionRecordsDeclaration`).

### Scraping the counters

`GatewayHandle::serve_metrics(address, bearer)` binds a second listener that serves the same
text as the owner's `/metrics` without the owner credential, so a monitoring stack never holds
the credential that can make paid model calls. The binary binds it when the deployment document
sets the top-level `metrics_listen`, a key beside `listen` (the scrape token is the top-level
`metrics_bearer_file`).

- It answers `GET` and `HEAD` on exactly `/metrics`. Any other method or path is
  `404 path-unknown`, so no model route, probe, public route or inspection path is reachable on
  it.
- With a bearer (the binary's `metrics_bearer_file`), a scrape presents that token as
  `authorization: Bearer <token>`, compared in constant time: none is `credential-absent`, a
  value that is not one bearer token `credential-malformed` and another token, the owner's
  included, `credential-rejected`, each `401`. Without one the listener is open: bind it only on
  an address a network policy guards.
- It serves before `mark_ready` and while draining, so a drain stays visible, and it stops with
  the gateway's shutdown or drop.
- The owner-guarded `/metrics` of the serving listener is unchanged.

### The dashboard

DECIDED 2026-11-04 (story:live-dashboard). `GET /dashboard` serves one self-contained page,
`/dashboard/snapshot` its JSON (`llm-gateway.dashboard/1`) and `/dashboard/events` the same JSON
as a server-sent event at once and after every change. They take the owner credential like
inspection (none on a gateway open on loopback) and serve while unready and while draining. The
record types are declared once in `dashboard.rs` (`SNAPSHOT_TYPES`);
`dashboard/src/generated/snapshot.ts` is generated from them and drift-checked by
`llm-gateway-docs generate --check`. The page is built from `dashboard/` (Vue, Vite) into
`crates/llm-gateway/src/dashboard.html`.

- **Calls** (the last `MAX_CALLS`; the page shows the newest 30 and the rest on request) with
  every attempt, its status, wait, `retry-after`, relayed answer headers and `error_body`;
  **in-flight** calls with the route and the upstream model id the call is sent as;
  **sessions** (the last `MAX_SESSIONS`); **routes**. A session and a route count `errors` and,
  apart from them, `cancelled`: calls the client left before any answer was written (status 0,
  no refusal) or while it was written; the **KPI** tiles; **subscription limits** per credential
  label.
- **Tokens per second.** A call's input rate is (uncached + cache read + cache creation input
  tokens) over its time to first token. Its output rate is its output tokens over the time after
  the first token, and only when it has at least `MIN_RATED_OUTPUT_TOKENS` (50) output tokens
  over at least `MIN_RATED_STREAMING_MS` (250 ms); otherwise it is `null`. A `null` rate is left
  out of every average, histogram and KPI.
- **Cache.** A route's hit rate is cache read over every prompt token, from the usage the
  answers report (`cache_read_input_tokens`, or `cached_tokens` in `prompt_tokens_details` or
  `input_tokens_details`); a call whose answer reports none has `null` cache fields, never an
  estimate. A vLLM server reports no cache fields in its answers, so for a hosted route on a
  Runpod pod's proxy address (`https://<pod>-<port>.proxy.runpod.net`) the binary reads the
  server's `/metrics` every 15 seconds with the route's key and hands the dashboard
  (`Dashboard::set_upstream`) the increase of `vllm:prefix_cache_hits_total` and
  `vllm:prefix_cache_queries_total` since it began reading, `vllm:num_requests_running`,
  `vllm:num_requests_waiting`, `vllm:kv_cache_usage_perc` and whether `<base path>/models`
  answered 200. The route row carries it as `upstream`. Such a route's cache figures are the
  server's (`cache_source` `vllm`): `cache_read_tokens` its hits, its prompt tokens its queries,
  `cache_hit_rate` the hits over the queries, and `cache_creation_tokens` `null`, because vLLM
  stores every computed block itself and reports no separate write; its calls and sessions keep
  a `null` cache read. The
  KPI `cache_hit_rate` and `llmgw_cache_hit_ratio` are every route's cache reads over every
  route's prompt tokens, both sources together, and `cache_routes` lists each route's figures.
- **Runpod account.** With a provider that names its Runpod API key (`api_key`, a keychain
  reference, or `api_key_file`, a file read under the trusted-file rules), the binary reads
  the account every 60 seconds (`Dashboard::set_provider`): balance, spend rate, the pods the
  gateway owns, and every other live pod counted in `other_pods`. A pod a hosted route reaches
  through its proxy address is also a row, marked `external`: it has no countdown and is never
  stopped or changed. Each owned pod's `shutdown` is the earliest of its model's idle stop (no
  call in flight, `idle_timeout_minutes` after the later of its last call and its start),
  `[watchdog].max_runtime_hours` after its start, and the time the balance falls under
  `[watchdog].min_balance_usd` at the current spend rate. A read of the account that fails
  keeps the last balance read on the page and logs one warning naming the provider, the host
  and the HTTP status or the connection's error, never the key; a read that got the balance but
  no complete pod listing sets the provider's `pods_unread`, shown as `pods not read` beside its
  read time, with no pod row and no other pod counted until a listing reads whole.

The page's series on `/metrics` follow the route series: token and cache counters, the
time-to-first-token, queue-wait and retry-wait histograms, `llmgw_cache_hit_ratio`,
`llmgw_tokens_per_second{direction}`, `llmgw_input_tps` and `llmgw_output_tps{route}`,
`llmgw_anthropic_utilization`, `llmgw_anthropic_reset_seconds` and
`llmgw_anthropic_status{window,credential}`, `llmgw_upstream_prefix_cache_hit_ratio`,
`llmgw_upstream_requests_running`, `llmgw_upstream_requests_waiting` and
`llmgw_upstream_kv_cache_usage_ratio{route}`, then the in-flight, client, status, attempt and
session series, and the provider's balance, pods and `llmgw_pod_shutdown_seconds{pod,limit}`. A
gauge is written only once it has a value. `tests/observability.rs` holds the order.

## Refusals

A refusal names its own stable code and a message this crate wrote. It quotes nothing the caller
sent: a rejected credential is never echoed back, and no upstream response text reaches a
refusal. A target's own answer is relayed, not refused, and is the target's text.

```json
{"error":{"code":"credential-absent","message":"no owner credential was presented"}}
```

One code carries more than its message: `model-not-pulled` (an Ollama model its server does not
list, [an Ollama upstream](#an-ollama-upstream)) is the message below, `: ` and the alias of
the relayed model the call was matched to, as its deployment declares it, never a byte copied
from the request.

```json
{"error":{"code":"model-not-pulled","message":"the model is not pulled on its Ollama server, and the gateway pulls no model: local"}}
```

| Code | Status | Message |
| --- | --- | --- |
| `body-incomplete` | 400 | the request body ended or stalled before it was complete |
| `body-not-allowed` | 400 | the inspection surface accepts no request body |
| `credential-absent` | 401 | no owner credential was presented |
| `credential-malformed` | 401 | the owner credential is not a bearer token |
| `credential-rejected` | 401 | the presented owner credential was rejected |
| `credential-review-unavailable` | 503 | the presented credential could not be reviewed; ask again |
| `body-not-json` | 400 | the request body is not one JSON object |
| `body-too-large` | 413 | the request body exceeds its byte bound |
| `call-unknown` | 404 | the gateway holds no call with that id |
| `context-exceeded` | 400 | prompt is too long: the input leaves no room for output in the model context window |
| `method-not-allowed` | 405 | this path does not accept that method |
| `model-absent` | 400 | the request body names no model |
| `model-cold-start` | 503 | the model is still starting; ask again after the retry-after delay |
| `model-not-pulled` | 503 | the model is not pulled on its Ollama server, and the gateway pulls no model |
| `model-stopped` | 503 | the model's pod is not running and a request does not start it; the owner wakes it |
| `model-unknown` | 404 | no such model |
| `overloaded` | 503 | the gateway is already serving its maximum concurrent requests |
| `path-unknown` | 404 | no such gateway resource |
| `request-malformed` | 400 | the request is not well-formed HTTP |
| `request-too-large` | 431 | the request head exceeds its byte bound |
| `route-unknown` | 404 | no such route alias |
| `spend-ceiling` | 503 | the model's pod was not started: it would pass a spend ceiling of its provider account, or that account's spend is unknown |
| `target-unavailable` | 503 | no model target is available |
| `tools-not-served` | 400 | the model does not serve tool calls |
| `server-tool-not-served` | 400 | this route is not Anthropic's: it serves no Anthropic server tool (web search, web fetch, code execution) and no tool without input_schema; send such a request to a claude model |
| `translation-invalid` | 400 | the request breaks a rule of the subset the gateway translates |
| `translation-opaque-state` | 400 | the request carries reasoning state the gateway did not seal for this route |
| `translation-too-large` | 502 | the model's answer passed a bound of the translated path, published in the gateway contract |
| `translation-unrepresentable` | 502 | the model's answer cannot be written on this wire |
| `translation-unsupported` | 400 | the request uses a field or value outside the subset the gateway translates |
| `unavailable` | 503 | the gateway is not ready to serve inspection |
| `upstream-failed` | 502 | the model target could not be reached or failed |
| `upstream-refused` | 502 | the model target refused the translated request |
| `wire-not-served` | 400 | the model is not served on this wire |

The lifecycle routes add no code of their own: a model with no lifecycle (a hosted endpoint, an
Ollama model or a route of the routes document) is `path-unknown`, an alias no relayed model has
`model-unknown`, and a wake the target source refuses `target-unavailable`, `spend-ceiling` or
`model-cold-start`, as its acquisition would be ([the lifecycle routes](#the-lifecycle-routes)).

A `target-unavailable` message may carry the target source's reason after the fixed text, as
`no model target is available: <reason>` (`RelayTargets::unavailable_reason`): the binary's
Runpod provider whose start read no complete pod listing answers `no model target is available:
provider <name> was not reachable at start`. The reason names no secret and quotes nothing a
client sent; the code and status do not change.

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
| `caller-session-bytes` | `MAX_CALLER_SESSION_BYTES` | 256 |
| `principal-bytes` | `MAX_PRINCIPAL_BYTES` | 128 |
| `targets-per-route` | `MAX_TARGETS_PER_ROUTE` | 64 |
| `routes` | `MAX_ROUTES` | 4096 |
| `digest-characters` | `DIGEST_CHARACTERS` | 64 |
| `request-head-bytes` | `DEFAULT_MAX_HEAD_BYTES` | 8192 |
| `concurrent-requests` | `DEFAULT_MAX_CONCURRENT_REQUESTS` | 64 |
| `request-body-bytes` | `MAX_REQUEST_BODY_BYTES` | 33554432 |
| `ended-calls-held` | `MAX_ENDED_CALLS` | 1024 |

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
`base-path-segments`, `static-header-value-bytes`, `caller-session-bytes`, `principal-bytes`,
`targets-per-route`,
`routes`, `request-head-bytes`, `concurrent-requests`, `request-body-bytes` and `ended-calls-held` are maxima; `shared-secret-bytes` is a minimum;
`digest-characters` is an exact length. `request-head-bytes` and `concurrent-requests` are
`GatewayConfig` defaults an embedding may override; they and `request-body-bytes` (32 MiB, as
llmgw) are measured by enforcement over a real socket rather than by reading a field.
`ended-calls-held` is measured over a socket too: after that many model calls have ended, plus
one, the first one's record is `call-unknown` ([call ids and call records](#call-ids-and-call-records)).

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

## Bounds of the service-account review

The bounds of the binary's TokenReview call (`crates/llm-gateway-cli/src/token_review.rs`,
`ReviewBounds::default`); the gateway crate holds none of them, so they are not in
[Bounds](#bounds). `sa_the_published_review_bounds_are_the_defaults` in
`crates/llm-gateway-cli/tests/token_review.rs` reads this table.

| Bound | Constant | Value |
| --- | --- | --- |
| `review-timeout-ms` | `ReviewBounds::timeout` | 5000 |
| `concurrent-reviews` | `ReviewBounds::concurrent` | 8 |
| `review-cache-entries` | `ReviewBounds::entries` | 1024 |
| `admission-cache-seconds` | `ReviewBounds::admission` | 60 |
| `rejection-cache-seconds` | `ReviewBounds::rejection` | 3 |

One review, connection, handshake, request and answer, ends within `review-timeout-ms` or is
`credential-review-unavailable`: each of those steps is given only what is left of the one
deadline the review starts with. A review asked while `concurrent-reviews` are under way is
`credential-review-unavailable` at once and sends nothing. An answer that sets `status.error` is
`credential-rejected`, whatever its `authenticated` says. Answers are kept by the SHA-256 of the
token, never the token: an admission for `admission-cache-seconds` and never past the token's
own `exp`, a rejection for `rejection-cache-seconds`, and an unavailable answer not at all. Past
`review-cache-entries` expired entries go first, then the rejection whose window ends soonest; a
rejection never evicts an admission (with every entry an admission, a new rejection is not
kept), and an admission evicts the admission whose window ends soonest only when no rejection
is left.

The pre-filter: before a review slot is taken, the token's claims are read without verifying
its signature. A token that is not three base64url segments around a JSON object, whose `exp`
has passed, or whose `aud` does not contain the configured audience is `credential-rejected`
without a review and without a call to the API server. The pre-filter can only refuse: a token
that passes it is still reviewed, and nothing read from its unverified claims admits it, names
its principal or keys the cache. What remains: a caller that knows the audience (it is in the
deployment document and in every agent's pod) can send distinct JWT-shaped tokens naming it and
a future `exp`. Each takes a review slot and an API server call, and while `concurrent-reviews`
are taken by them a listed caller whose admission is not kept is
`credential-review-unavailable`. The mode belongs behind a NetworkPolicy that admits only the
agents' pods.

`token_review.ca_file` is read once, at start: a rotated cluster CA takes a restart.
`token_review.credential_file` is read again at every review.

## Bounds of the status probe

The bounds of the binary's status probe of a hosted model or route
(`crates/llm-gateway-cli/src/hosted.rs`, `ProbeBounds::default`); the gateway crate holds none of
them, so they are not in [Bounds](#bounds).
`route_status_the_published_probe_bounds_are_the_defaults` in that file reads this table.

| Bound | Constant | Value |
| --- | --- | --- |
| `status-probe-timeout-ms` | `ProbeBounds::timeout` | 5000 |
| `status-reuse-seconds` | `ProbeBounds::reuse` | 10 |
| `ollama-list-max-bytes` | `OLLAMA_LIST_MAX_BYTES` | 1048576 |

One probe, name lookup, connection, handshake, request and first final status, ends within
`status-probe-timeout-ms` or is `unreachable`/`no-answer`. The lookup and the connection run on
a thread of their own, so a lookup that does not return ends the probe at its deadline too. Such
a lookup is left to end on its own, at most one per model: while it runs, a status read of that
model starts no second one, waits for it until its own deadline and is `no-answer` if it has not
ended; once it has ended, what it found is dropped and the next probe looks the name up again.
A probe's result is reused for every
status read of that model until `status-reuse-seconds` have passed since the probe ended; the
next read after that probes again. `GET /v1/status` asks every model side by side, so it is
answered within `status-probe-timeout-ms` and one second however many endpoints are silent.

An Ollama model's status read, `GET /api/tags` and `GET /api/ps` on two connections, ends within
one `status-probe-timeout-ms` for both, and is reused as a probe is. Each list's body, framed by
`content-length`, chunked or read to the end of the connection, is at most
`ollama-list-max-bytes`; a longer one is `no-answer`.

## Bounds of the translated path

The bounds llm 0.24.0 holds a translated answer to, per upstream stream (`llm_http::sse`, and
llm-core's `MAX_TOOL_ARGUMENT_BYTES`, read by the binary's translator); the gateway crate holds none of them, so they
are not in [Bounds](#bounds). An answer at a bound is written whole; one past it ends with the
wire's terminal error carrying `translation-too-large`'s message, or, before any output, is the
refusal `translation-too-large` (502). An outcome past llm's own bounds (4096 items, 16 MiB
encoded) is refused the same way.

| Bound | Constant | Value |
| --- | --- | --- |
| `translated-stream-events` | `MAX_EVENTS` | 65536 |
| `translated-stream-bytes` | `MAX_STREAM_BYTES` | 33554432 |
| `translated-line-bytes` | `MAX_EVENT_BYTES` | 1048576 |
| `translated-event-bytes` | `MAX_EVENT_BYTES` | 1048576 |
| `translated-tool-argument-bytes` | `MAX_TOOL_ARGUMENT_BYTES` | 65536 |

All five are maxima: the events of one stream; its bytes; the bytes of one line of it, comment
lines included; the bytes of one event's data, its `data:` lines joined with `\n` (llm-http
applies `MAX_EVENT_BYTES` to both); and the bytes of one tool call's arguments (llm-core's
`MAX_TOOL_ARGUMENT_BYTES`, which the messages decoder holds streamed fragments to and every
outcome its parsed arguments).
`translation_bound_every_published_bound_of_the_translated_path_flips_at_its_number` in
`crates/llm-gateway-cli/tests/translation_bounds.rs` reads this table and measures each row at
its number and one past it.

## Verification

The [verification record](verification/gateway.md) states the measured coverage, the mutations
that were shown to kill named tests, and the qualification boundaries that remain.
