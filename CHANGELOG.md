# Changelog

Every release is tagged `<version>` and publishes the source and, from the first release after
0.9.0, the gateway image; the crates are not published. The
[GitHub Releases](https://github.com/beyond10x/llm-gateway/releases) carry the same notes.
Nothing in any release is deployed or qualified against a live client.

## Unreleased

### Changed

- `RestRunpod` calls Runpod's REST API v2 (`https://api.runpod.io/v2`) where it called `https://rest.runpod.io/v1`: `GET /pods` follows `pagination.nextCursor` for at most 64 pages, and a listing is complete only when every page read; `POST /pods` sends `image` and `gpu: {id, count}`; `DELETE /pods/<id>`; the balance is `GET /billing/balance`, and Runpod's GraphQL `myself { clientBalance }` only when that answers 403 or 404; spend is `GET /billing/pods?bucketSize=day`. A listing whose first page fails is an error (`list_pods` returns `Err`), one whose later page fails is incomplete. `RestRunpod::last_failure` (and `RunpodAccount::last_failure`) names the host and the cause (`CallFailure`, `FailureCause`: HTTP status, connection error, no TLS, a host the transport does not call, no answer), never the key. `crates/llm-runpod/tests/fixture/runpod-openapi-v2.json` holds the subset of the v2 OpenAPI document the client reads, written by hand and marked NOT DOWNLOADED, with its SHA-256.
- `AccountSnapshot` gains the public field `pods_read`, and `ProviderPanel` the public field `pods_unread` (record field `pods_unread`, shown as "pods not read"): a caller that builds either by literal no longer compiles. `llm_gateway_cli::Provider` gains `management_url` and `proxy_url`. The next release takes a minor step.
- The binary sends every call to a Runpod pod but a token count streamed (`stream: true`, and on the chat wire `stream_options.include_usage`), and answers a client that did not stream with the one body assembled from the events (`EventAssembler`, at most 16 MiB of events), so an answer that takes longer than Runpod's proxy allows a silent connection completes. A stream cut before its end, or the proxy's `524`, ends the call `UpstreamCut`; a stream that assembles to no answer is `upstream-failed`.
- A Runpod provider whose start read no complete pod listing answers its models `target-unavailable` with the message `no model target is available: provider <name> was not reachable at start`, and its start warning names the provider, the host and the cause. A pod that was ready at its last observation keeps serving while the listing fails outright. A failed account read keeps the last balance on the dashboard and logs one warning; the watchdog counts a read without a complete pod listing as an unreadable account.

### Added

- `RelayTarget::streamed`, `RelayTargets::unavailable_reason`, `Relay::with_assembler` and the traits `StreamAssembler` and `Assembly`; `EventReader` is public (`feed`, `take`). All trait methods have defaults: an existing target or target source compiles unchanged and is relayed as before.
- `providers.<name>.management_url` (default `https://api.runpod.io/v2`; `https://`, or `http://` to a loopback host; read only beside `api_key`) and `providers.<name>.proxy_url` (default `https://{pod_id}-{port}.proxy.runpod.net`; `https://`, placeholders `{pod_id}` and `{port}`, at most 512 bytes) in the deployment document. A value outside the rule is `config:value` naming the key. `ProxyUrl` in `llm-runpod`; `RunpodPool::with_proxy_url`.
- `providers.<name>.api_key_file` in the deployment document: the path of a file holding the account's Runpod API key, in place of the keychain `api_key`. It is read once at start under the rules of `vllm_api_key_file` (no final symbolic link, a regular file owned by the user or root, mode & 0o077 refused, at most 4098 bytes, one printable token of at most 4096 bytes once trailing whitespace is trimmed); a broken rule refuses the start before any request reaches Runpod as `endpoint-credential:<rule>`, its message starting `providers.<name>.api_key_file:` and naming neither the path nor the key. It is refused beside `api_key` and beside `connectors` (`config:value`), and counts as `api_key` everywhere else: one keyed provider, `management_url`, the dashboard's account read and the watchdog. `llm_gateway_cli::Provider` gains the public field `api_key_file`: a caller that builds a `Provider` by literal no longer compiles.

## 0.26.0 (2026-11-26)

### Changed

- `metrics_listen` may equal `listen` when both name port 0: the system picks a port for each. Any other equal pair is still `config:value`.
- The gate's test step is `task test` (`cargo run --quiet --locked -p b10x-llm-gateway-xtask -- test`), locally and in CI: it builds the tests (`cargo nextest run --workspace --locked --no-run`, untimed, a CI step of its own), then runs them with cargo-nextest, each in a process of its own, kills a test over 60 s and counts it failed, fails when the run takes over 120 s of wall clock, and prints the ten slowest tests. It needs `cargo-nextest` 0.9.143 or newer on `PATH`; `.config/nextest.toml` refuses an older one, and CI installs 0.9.143. Each crate builds one integration-test binary from `tests/main.rs`: 16 test executables where there were 128, and no doc-test run (the workspace has no doc test). `cargo test -p <crate> --test <file>` becomes `cargo nextest run --locked -p <crate> <file>::`; `cargo test` now runs a crate's tests in one process, where 4 tests that measure their process or a time bound fail.
- No test chooses a port and releases it before use (the binary's, the hosted status probe's, the Runpod readiness probe's): a document names port 0 and the test reads the bound port from the binary's lines, and a test that needs a port where nothing answers holds that port.
- `GET /metrics` serves three more process-wide series after `llmgw_pod_start_spend_refusals_total`: `llmgw_interaction_records_dropped_total`, `llmgw_interaction_records_skipped_total` and `llmgw_interaction_records_failed_total`, all 0 until an embedding counts them. No public struct gained a field. The next release takes a minor step.
- `llm_gateway_cli::Deployment` gains the public field `interaction_records` (`InteractionRecordsConfig`): a caller that builds a `Deployment` by literal no longer compiles. The next release takes a minor step.
- The interaction records link `sessionspec-records` and `sessionspec-types` at tag 0.1.1 (schema version 4 still): a held session keeps at most 4,096 response ids, 4,096 conversation ids and 4,096 spawn inputs, and a call whose link was evicted starts a new agent with no parent. A sessionspec of 0.1.1 or later that opens a store of another schema version keeps its wire records; the gateway still refuses a store of another version at start and writes a file of its own.

### Added

- Every call that is given an id (relayed, translated, passthrough or refused; never a token count, a shed or an unauthenticated request) hands one `FinishedCall` to the `FinishedCalls` port an embedding composes with `Relay::with_finished_calls`: its id, the dashboard's session hash and `Harness`, path, inbound wire, route, upstream, attempts, queue wait, time to first byte, the record's cost, its `CallEnding` and its start and end. It maps onto sessionspec's `GatewayFacts`. The dashboard reads its finished call from the same value.
- `Relay::with_call_bytes(bound)`: the port also gets the call's bytes, `CallBytes::Copied` with the request body and the answer as the client received it (`AnswerBytes::Body`, or `AnswerBytes::Events` with each event's `data:` payload for a stream), or `CallBytes::OverBound` with both sizes when they pass `bound`. An answer with a content coding is copied decoded by the relay's `BodyDecoder`, and the bound counts its decoded bytes; one that does not decode whole is `CallBytes::Undecodable` with both sizes and its coding. Without a bound no byte is copied. The client's answer is unchanged.
- `Metrics::count_interaction_record_dropped`, `Metrics::count_interaction_record_skipped` and `Metrics::count_interaction_record_failed`, which count the three new series.
- `[interaction_records]` in the deployment document (`path`, required; `queue`, default 1024; `call_bytes`, default 8 MiB; `sessions_held`, default 4096): the binary lifts every finished call into sessionspec's session records with `sessionspec-records` 0.1.1 and appends them to that SQLite file on one writer thread, which `sessionspec serve` reads. No message text is stored. A call that finds the queue full counts `llmgw_interaction_records_dropped_total`; one over `call_bytes`, whose answer was not decoded, or whose request the lift cannot read (it yields no step) counts `llmgw_interaction_records_skipped_total`; one the writer failed to store counts `llmgw_interaction_records_failed_total`, with at most one warning per second naming how many calls failed since the last. A refused call is an incomplete turn; a token count makes no record. A path whose directory is missing, a file that is no session store, a store whose schema row is not exactly this version (compared as sessionspec compares it) and a store another process holds locked refuse the start with `config:value` naming `interaction_records.path` and the reason, and the file is left as it was; a file holding only sessionspec's `meta` table without a schema row is a new store. At SIGINT or SIGTERM the binary waits at most 2 s for the writer to store what is queued, and counts the calls still queued and those failed since the last warning on one line before the stop line. Without the table nothing is copied, no writer runs and no file is created. Guide: Record agent sessions.

- The binary writes `b10x-llm-gateway: metrics listening on <address>` once, before `listening on`, when the document sets `metrics_listen`, with the address that listener bound, so a `metrics_listen` on port 0 reports the port the system picked.

### Added

- The dashboard and the setup page have a Light, Dark and System theme switch in the header, kept in the browser's local storage and applied before the first paint; both palettes keep text at 4.5:1 and controls at 3:1, every control shows a focus ring, a "Skip to content" link leads each page, and reduced motion turns transitions off.
- The dashboard shows the token flow in its own "Token flow" panel at the top of the page, above Overview, 180 px high (140 px on a narrow screen): a line from each active session (an in-flight call, or seen in the last five minutes), labelled with the first 8 characters of its id and its client, to each route it calls, each new call's input and output tokens moving along it on a log scale, a slow pulse while a call is in flight, at most 30 frames a second, paused in a hidden tab and still under reduced motion. Its heading line counts the active sessions and their routes, a caption says what a line, a blue line and the dots mean and which way input and output tokens move (under reduced motion, that motion is off), every line, node and dot is drawn at 3:1 or more against the panel and every label at 4.5:1, the panel keeps one height whether sessions come or go, and it says "No active sessions." when there are none. It reads only the snapshot the page already receives.
- The Gate workflow has a `dashboard` job: on a push to `main`, and on a pull request whose diff reaches the dashboard, it type-checks the dashboard, runs its unit tests, checks that the built page equals the committed one and runs the Playwright gate; `task dashboard-ux` runs the same steps locally.

### Changed

- The dashboard is specified by one ess-ui/1 document, `dashboard/ui/llm-gateway.ui.yaml`, checked with `ess ui check` in `task dashboard-ux` and the Gate workflow, and drawn by the page's own renderer from it; the page is still one embedded file and looks and behaves as before, except that the Provider panel names the provider as a "Name" figure instead of in its heading.
- The dashboard's subscription limit tiles wrap a value only between words or after an underscore, never inside a word; the status `allowed_warning` reads "allowed (warning)", and every status, overage, claim and overage reason keeps its raw header value in its tooltip.
- The dashboard's Recent calls table fits a 1280 px wide window without sideways scrolling: its column headings, its flags and the upstream model and provider lines under a route wrap onto more lines instead of widening the table.
- The dashboard says a watchdog heartbeat older than 5 minutes is stale in words beside its age ("11m 0s ago · stale (over 5 min)"), not in red alone.

## 0.25.0 (2026-11-26)

### Changed

- Breaking: a model call no longer starts a Runpod pod. A call to a model whose pod is stopped or stopping is refused at once with the new `model-stopped` (503, no `retry-after`, outcome `refused`): no pod is created and no spend is read. Only the owner's wake (`POST /v1/models/{alias}/wake`) starts the pod; a call after the wake is held on the starting pod and relayed once it serves, or `model-cold-start` when its hold passes, as before, and a pod that fails while a call is held starts no replacement. To restore the old behaviour for a model, set `start_on_request = true` in its `[models.<alias>]` table, `daily_cap_usd` on its provider, and `price_per_hour_usd` on every model the document declares under `[models]` (a declared cap requires it of each).
- The `upstream-failed` message is now "the model target could not be reached or failed": it no longer says the next request asks for a replacement, which is not so for a model without `start_on_request`.
- The status of a hosted model and of a route of the routes document is a probe of its endpoint (`GET <base path>/models` with its credential), sent only when a status is read, at most one per model at a time, within 5 s, its name lookup included, its result reused for 10 s: `ready` on a 2xx answer (and on 401 or 403 for a `client` endpoint, probed without a credential), otherwise `unreachable`. It was `path-unknown`. Wake stays `path-unknown` for both, and a Bedrock model's status stays `path-unknown`.
- Breaking for a library caller that builds these structs by their fields: `UsageRecord` gained `cost_usd`, and `Model`, `Hosted` and `Bedrock` of `b10x-llm-gateway-cli` gained `token_price`.
- A route's `price` in the routes document is no longer refused: it prices the route's calls.
- Breaking for library callers: `UsageRecord` has a new field `call`, the call's id; code that builds a `UsageRecord` with a struct literal must set it.
- The binary's `usage` event names the call in its field `call`, and carries the token counters the record has as `input_tokens`, `output_tokens`, `cache_read_tokens` and `cache_creation_tokens`; a counter the record does not carry is no field.

### Added

- `GET /v1/status` (owner; `HEAD` too): one list, format `llm-gateway.status/1`, of every relayed model in alias order with its `kind` (`pod`, `hosted`, `route`, `bedrock`), `wires`, `phase`, `sub_phase`, `profile` and `callable`, whether a call sent now is relayed, held on a pod that is starting or serving, or starts the pod (`true` for `ready` and `starting`, and for `stopped` where a call starts the pod: in the binary only a Runpod model with `start_on_request` under its own provider's enforced daily cap, so a model whose call is `model-stopped` is `false`; `false` for `unreachable`, and for `stopping` with or without `start_on_request`, where a call would be refused or wait on a stop still owed). A Bedrock model has neither `phase` nor `callable`. Every status is asked side by side. A status read starts, wakes and invalidates nothing and makes no usage record.
- The phase `unreachable`, with `sub_phase` `no-answer`, `credential-refused` or `status-<code>`.
- `ModelKind` and `RelayModel::with_kind`; `RelayModel::with_no_call_start`, which declares that a call does not start the model's target, so the status list calls it not callable while `stopped` (an undeclared model keeps `stopped` callable); `ProbeBounds`, `Resolver` and `HostedTargets::with_probe_bounds`/`with_probe_clock`/`with_resolver`.
- `[hosted.<alias>] kind = "ollama"`: a model on an Ollama server. `base_url` is the server's root, `http://127.0.0.1:11434` by default, on any host and with no path; `wires` defaults to all three, each relayed to Ollama's own path (`/v1/chat/completions`, `/v1/responses`, `/v1/messages`), a wire left out translated onto the first one named; `upstream_model` is the Ollama model name (a name without a tag is `<name>:latest`); `auth_kind` is `anonymous` only and a credential key is refused; `model_prefix` is refused, since the model's status is read for `upstream_model` alone; `billing_kind` defaults to `self-hosted`. Its kind is `ollama` in `GET /v1/status`, and the route inventory lists it under provider `ollama`, account `local`. `HostedKind`, `OLLAMA_DEFAULT_BASE_URL`, `HostedEndpoint::with_ollama`, `ModelKind::Ollama`. Guide: serve a local Ollama model.
- An Ollama model's status is read from `GET /api/tags`, then `GET /api/ps`, within the status probe's 5 s and reused for 10 s: `ready` when loaded, `stopped` `not-loaded` when pulled and not loaded (callable: the first call loads it), `unreachable` `model-not-pulled` when the server does not list it, `unreachable` `no-answer` when it does not answer. Each list's body is at most `ollama-list-max-bytes` (1048576, `OLLAMA_LIST_MAX_BYTES`). Wake is `path-unknown`.
- The refusal `model-not-pulled` (503, no `retry-after`, outcome `refused`) for a call to an Ollama model its server does not list; its message names the alias. `TargetRefusal::NotPulled`. The gateway sends an Ollama server no pull and no request that changes what it stores.
- `[models.<alias>] start_on_request` (`true` or `false`, default `false`): `true` lets a call start the model's pod as before. It is refused at start as `config:value`, naming the model and the missing cap, while the model's provider declares no `daily_cap_usd`.
- The refusal code `model-stopped` (503): the model's pod is not running and a request does not start it; the owner wakes it.
- `[models.<alias>.token_price]` and `[hosted.<alias>.token_price]` (every hosted kind): what a model's tokens cost in USD per 1000000 tokens, `input` and `output` required, `cache_read` and `cache_creation` optional, each a quoted decimal string. A value llm's `Amount` refuses, a negative one, a number, a missing `input` or `output` and an unknown key are refused `config:value` naming the key and quoting no value.
- A route's `price` in the routes document is read as llm writes it: `input`, `cached_input`, `cache_creation_input` and `output` price the same four classes; a currency other than `USD` is refused `config:value` naming `routes.<route>.price.currency`.
- `UsageRecord::cost_usd`: what a call cost, priced by llm's `PriceBook::quote` (crate `llm-cost` 0.24.0, a new dependency of `b10x-llm-gateway-cli`), as llm writes an amount. Present only for a call relayed to its end (not cut short by its client or its upstream, and not ended in its wire's terminal error, on every relay path) whose model declares a price, whose answer reported its input and output tokens, and whose every class with a count above 0 has a rate; never `0`, never an estimate. The binary's `usage` event carries it as `cost_usd`, and the record `GET /v1/calls/{id}` answers as its key `cost_usd` after the token keys, a JSON string such as `"0.000138"`, absent when the call has no cost.
- `CallPricing`, the port a relayed model's calls are priced through, and `RelayModel::with_pricing`; `TokenPrice` in `b10x-llm-gateway-cli`, a declared price and its `CallPricing`.
- `UsageScanner::terminal_error`: whether the answer read so far carried its wire's terminal error (a top-level object whose `type` is `error` or `response.failed`, or whose `error` is an object).
- Every answer to a model call that passed authentication, or needed none, carries `x-llm-gateway-call: <id>`: a relayed answer, a translated one, a passthrough answer and a refusal. An id is 1 to 64 characters of `A-Z a-z 0-9 -` and does not repeat after a restart. A shed request and one that fails authentication get none.
- `GET /v1/calls/{id}` (the credential `GET /v1/launch` takes; `HEAD` too): the call's record, format `llm-gateway.call/1`, `in-flight` while its answer is written and `ended` with the usage record's values after its last byte. A value the usage record does not carry is an absent key. The gateway holds every call in flight and the last 1024 ended calls (bound `ended-calls-held`).
- The refusal code `call-unknown` (404): the gateway holds no call with that id, whether it is of another form, never given out or no longer held.

### Fixed

- The token counters no longer read a tool call's arguments. Before, a member named `input_tokens`, `output_tokens`, `prompt_tokens`, `completion_tokens`, `cached_tokens`, `cache_read_input_tokens` or `cache_creation_input_tokens` anywhere in an answer counted, so a Messages `tool_use` whose `input` object carried such a name set the usage record's counters and the dashboard's token figures (in 0.24.0). They are now read from the answer's usage object only: the `usage` of the answer, of its `message` or of its `response`, and its `prompt_tokens_details` or `input_tokens_details`.

## 0.24.0 (2026-11-21)

### Added

- `ingress_auth = "service-account-token"` and `[token_review]`: a model call is admitted with the owner secret, compared first, or with a Kubernetes service-account token the cluster's TokenReview API reports authenticated for the configured `audience` and naming a service account on `principals`. `api`, `ca_file` and `credential_file` default to the in-cluster API server and the pod's mounted service-account files. A rejected token is `credential-rejected`; a review with no answer is the new `credential-review-unavailable` (503); neither admits. Reviews are kept by the SHA-256 of the token, admissions for at most 60 s and never past the token's expiry, rejections for 3 s; at most 8 run at once, each within 5 s. The admitted service account is the `principal` of the usage record and of the `usage` event; no token or review body is logged. Before a review the token's own claims are read unverified, only to refuse: not a JWT, expired, or an `aud` without the configured audience is `credential-rejected` with no API server call. An answer that sets `status.error` is a rejection. One deadline bounds the whole review. A rejection never evicts a kept admission. `ca_file` is read once at start, `credential_file` at every review. Startup refusals `token-review:<rule>`. `bearer` and `none` are unchanged. Guide: admit service-account tokens.
- `TranslatedAnswer::failed`, defaulted: a translator tells the relay that an answer whose head was written ended in its wire's terminal error.

### Fixed

- A translated Messages answer names the alias the client asked for as its `model`, in `message_start` (absent before) and in the non-streamed `message` (the target's reported model before).
- A translated answer that ends in its wire's `error` event after its head is recorded `upstream_error` in the usage record and `llmgw_requests_total`, not `ok`; it stays `Relayed`, status 200.
- A Messages client's `cache_control` marks reach a Bedrock `anthropic-invoke` model on the blocks and tools it marked, with their lifetime, and no breakpoint is added. Before, a rolling breakpoint was added on the last message's last block even when the client marked only its system block. A request without a mark keeps llm's two breakpoints, the system block and the last message's last block.
- Signed thinking without text, which Claude Haiku 5.5 streams through `InvokeModel` at a low effort, reaches a Messages client as a thinking block before the answer; before, the answer ended in an `error` event after its text.

### Changed

- llm 0.24.0 (was 0.20.0) for every llm client crate. `Capabilities` is built with `Capabilities::text` and its `with_*` setters. llm's decoders now read structured output (`response_format`, `text.format`, `output_config.format`) and a user image; the gateway declares both `false` in the capabilities it translates under, a Bedrock model's included, so a translated path still refuses them `translation-unsupported` before any target is asked for, now logged as `binding does not take structured output` or `binding does not take image input`. `text.format` and `output_config.format` leave the list of paths the `translation refused` line names as written. llm's new stream events (`MessageBoundary`, `OpaqueItem`) reach llm's own encoder for the client's wire, as every event does. `docs/gateway.md` "The request".

## 0.23.0 (2026-11-21)

### Added

- `b10x-llm-gateway serve [--config <file>]` serves the default document, `$XDG_CONFIG_HOME/llm-gateway/config.toml` (else `~/.config/llm-gateway/config.toml`), when no file is named; the closed command line `b10x-llm-gateway <file>` is unchanged.
- Keychain references: `keychain:<service>/<account>` names a credential in the OS keychain (`secret-tool` on Linux, `security` on macOS) in place of a file, for `[providers.<name>].api_key` (the Runpod API key, which runs the provider's pool over `RestRunpod`), `[models.<alias>].vllm_api_key`, `[hosted.<alias>].credential` and a routes document endpoint's secret. Each is read once at start; a missing or unreadable entry refuses the start as `endpoint-credential:<rule>` or `vllm-api-key:<rule>`, quoting no value.
- `b10x-llm-gateway watchdog`, a second process that terminates the pods the gateway owns when the gateway cannot: a stale or missing heartbeat, the balance under `min_balance_usd`, the runtime past `max_runtime_hours`, a pod not ready for `not_ready_minutes`, or an idle pod. With a `[watchdog]` table `serve` writes the heartbeat (`llm-gateway.heartbeat/1`), carrying each route's last call and calls in flight. Only pods named `b10x-llm-*` with the gateway's owner tag are ever terminated. systemd user units in `deploy/systemd/`.
- `auth_kind = "client"`: a hosted route to `https://api.anthropic.com`, behind `ingress_auth = "none"`, that forwards the client's own `authorization` and `x-api-key` and the request as the client wrote it, `accept-encoding` included, so Claude Code on a Claude subscription runs through a local gateway. `model_prefix` serves every model name starting with it; `newest` sends the alias as the newest model of a family the client's credential lists. A content-encoded answer reaches the client unchanged and a decoded copy (`gzip`, `deflate`, `br`, `zstd`) gives the usage. A 429, 502, 503 or 529 is retried before the first byte reaches the client, after `retry-after` or a 1, 2, 4, 8 s backoff with jitter capped at 30 s, within `retry_max_attempts` (4) and `retry_max_wait` (60 s), never on `x-should-retry: false`, and never for a client that sends `x-stainless-retry-count`, which retries on its own. Every non-2xx attempt keeps its decoded body, cut to 2048 bytes and redacted, as `error_body`.
- The dashboard: `GET /dashboard` (a Vue page built into one HTML file), `/dashboard/snapshot` and `/dashboard/events`. It shows calls with every attempt, in-flight calls, sessions, routes, KPI tiles (tokens, tokens per second over the last minute, cache hit rate), the subscription limits Anthropic reports per credential label (the first eight hex digits of the credential's SHA-256), and the Runpod account: balance, spend rate, owned pods with their automatic shutdown countdown, and a pod a hosted route reaches through its Runpod proxy address, marked external and never stopped. Input tokens per second count cached input; output tokens per second is rated only with at least 50 tokens over at least 250 ms. For a hosted route on a Runpod proxy address, whose vLLM answers carry no cache fields, the server's `/metrics` gives the route's cache figures (its prefix-cache hits over its queries, marked vLLM, and counted in the overall hit rate), running and waiting requests and KV cache usage. New series on `/metrics`, in `docs/gateway.md` "The dashboard".
- `GET /llms.txt` and the browser's `GET /` show the launcher commands; `GET /v1/launch` is the document the launchers read.
- `b10x-llm-gateway claude` starts Claude Code against the running gateway with a settings overlay (base URL, sub-agent route, `/model` picker, auto-compact windows; no credential) and appends `[launch.claude].subagent_instruction` with `--append-system-prompt`, joined after the user's own text. `b10x-llm-gateway codex` starts Codex with the gateway as a `-c` provider override. `[launch.claude]` and `[launch.codex]` in the deployment document.
- `RestRunpod`, a direct Runpod transport for a local gateway: Runpod's REST API (`https://rest.runpod.io/v1`) called with an API key the process holds, over the TLS connection the embedding hands in (the crate still links no TLS client), in minimal HTTP/1.1 with `Connection: close`, answers bounded at 8 MiB. `list_pods` is `GET /pods`; `create_pod` is `POST /pods` with the `pod.create` body unwrapped, Runpod's "no instances currently available" 4xx is `NoCapacity`, another 4xx `Refused`, and no answer, a 5xx or a timeout is resolved once by `GET /pods?name=` and the request tag, never resent; `terminate_pod` is `DELETE /pods/<id>`; spend is `GET /billing/pods?bucketSize=day`. The key is redacted in `Debug`, written only into a request buffer overwritten on drop, and appears in no answer or error. `RunpodAccount` (`AccountSnapshot`, `PodSummary`) reads the balance and spend rate from Runpod's GraphQL `myself` and the pods from `GET /pods`, for a watchdog or dashboard; `EmulatedRunpod` implements it with scripted values. `crates/llm-runpod/tests/rest.rs` drives it against a loopback fixture. `docs/design/runpod-clients.md` D1: connectors stays the path for a cluster.
- Every created pod's placement is read back (`RunpodTransport::pod_placement`, `PodPlacement`: `GET /pods/<id>?includeMachine=true`, or `pods.list` with `includeMachine` through connectors). Unless `machine.secureCloud` is `true` and `interruptible` is not `true` (an absent `interruptible` counts as false, as Runpod omits it), the pod is terminated at once, no further GPU of the profile is tried and the start is refused; a terminate whose answer is lost is sent at most three times, and a pod still standing is never reported ready and is left to the orphan sweep. `crates/llm-runpod/tests/adversary_secure_cloud.rs`.
- A Runpod model's image must start with `vllm/vllm-openai:` and name a tag (`ALLOWED_IMAGE_PREFIX`); any other is refused `image-not-allowed` (`ConfigError::ImageNotAllowed`), which the gateway reports `config:value`.

### Changed

- A reasoning effort now reaches an `anthropic-invoke` Bedrock model as `output_config.effort`, from every client wire, instead of being dropped. `[hosted.<alias>] reasoning_efforts` (Bedrock `anthropic-invoke` only; default `low`, `medium`, `high`, `xhigh`, `max`; `[]` for none) lists what the model takes; any other effort is refused `translation-unsupported` before anything is sent. A deployment whose model takes no effort or fewer levels declares the list. A `converse` model still drops an effort, parallel tool calls and non-Anthropic thinking, as `docs/gateway.md` says why.

### Removed

- Community Cloud: `CloudType::Community` and the document's `cloud_type = "COMMUNITY"` are gone. A document naming `COMMUNITY` is refused `config:schema`; every create sends `cloudType: SECURE` and `interruptible: false`.

## 0.22.0 (2026-10-30)

### Added

- `task check` and CI compare `spec/` with the specification at the merge base (`b10x-llm-gateway-conformance diff`, `task spec-diff`) using `ess verify diff --fail-on breaking-or-unknown`; a breaking or unclassified change fails the gate unless `contracts/diff-acknowledgements.json`, bound to both endpoint digests, acknowledges it. CI checks out the whole history and installs `ess` before the Rust tests.
- The hosting ownership state machine is the ESS lifecycle of `llm-gateway.hosting.OwnedResource`: `Phase`'s states and the 21 pairs of `TRANSITIONS` as twelve named transitions, each moved by a command outcome (`Declare`, `Provision`, `Retry`, `ObserveListed`, `ObserveTakenOver`, `ObserveUnlisted`, `RequestStop`, `ConfirmStopped`, `Cancel`) that the conformance target answers through `Controller::apply`, and read through the view `llm-gateway.hosting.OwnedResources`, which publishes every field of the record. `Provision` and `Retry` on a `Disowned` record answer `stale-epoch`, because `ObserveTakenOver` moves the lease to a successor at a newer epoch; a listing carries the resource of every submitted create, one answered about another account's resource included. Every lifecycle command but `Provision` and `Retry` answers `invalid-identifier` for an identifier that is empty, holds a space or is over 256 bytes, the stop evidence included; ESS 0.77.0 admits an authored input that breaks its declared type, and builds no witness for that refusal beside the `answer` guards of `Provision` and `Retry`, where such an identifier stays a target error. `Declare` records its `id` as the record's `deployment_spec_id`. The record's `phase` field is gone, because its lifecycle state is the phase; `ess verify diff` classes the change breaking and `contracts/diff-acknowledgements.json` acknowledges its 32 changes. A test holds the lifecycle equal to `TRANSITIONS`, and `ess verify conform mutate --class from-drop --class transition-to` scores a mutant for every transition with none surviving (`docs/verification/2026-10-30-hosting-lifecycle-mutation.md`). The suite's floor is 395 scenarios, the two translation scenarios included. `docs/hosting.md` "The table as a declared lifecycle".

### Changed

- llm 0.20.0 (was 0.19.0) for every llm client crate. A Chat Completions upstream's `refusal` text now reaches the translator as llm's `StreamEvent::RefusalDelta`; the gateway hands every event to llm's own encoder for the client's wire, so it needs no arm of its own: a chat client receives it as `refusal`, and a Responses or Messages client receives no delta for it.
- `GET /` profiles a client for a model whose upstream speaks another wire when the relay's translator admits the client's requests to it: the binary prepares Codex's and Claude Code's recorded first turns for each such model once, at start, and profiles the client where llm carries the turn, so a client is handed only models its requests reach without a `translation-*` refusal. A translated-only Bedrock model, which relays no client wire, is profiled only for a client whose first turn passes llm's Bedrock check, no longer for every client. No Loom turn is recorded, so a Loom run is still profiled only for models that declare `chat`. A Claude Code block for a model reached only by translation also exports `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1'`, since every translated path refuses Claude Code's structured output (`output_config.format`) `translation-unsupported`; that the variable stops it is from Claude Code's published gateway protocol, not observed against a live client. When no model serves a client, the answer names only what was checked: for Codex and Claude Code, that no model with tool calling parsed relays the wire or admits the recorded first turn translated; for Loom, that none relays the chat wire and no Loom turn is recorded to try a translated one. It no longer says that none declares the wire. The recorded first turns the binary embeds move to `crates/llm-gateway-cli/src/admission/`. The translator port gains `Translator::admits`, default `false`. Two conformance scenarios hold it. `docs/gateway.md` "The public routes".

### Fixed

- A credential is no longer left in memory freed unzeroed by a buffer that grew around it: the client's request head, the trusted-file reader, a key read from a file, the hosted probe's request, `credential set`'s standard input and prompt, the owner-token hash, the owner secret file's text and a Bedrock token that fails its version check are each sized before the credential enters or moved by an overwrite, and overwritten, spare capacity included, on every path that lets them go, refusals included. Best effort, as before: no `zeroize` dependency. `docs/verification/2026-10-30-credential-buffers-zeroed.md`.

## 0.21.0 (2026-10-30)

### Added

- A conformance scenario relays an answer whose target holds its head back 400 ms, four times the gateway's 100 ms client watch, to a request step with default options, so it fails if the runner's request step half-closes by default again. The runner's pod answer takes `delay_ms`, at most 2000. The suite's floor is 292 scenarios.

### Changed

- The `translation refused` line writes a key the client chose outside the subset as `?` after its location (`?`, `text.?`, `messages[].content[].?`), whatever its shape, so a key shaped as a secret is not logged. Names from a fixed list stay named: llm's own (``chat request field `n` ``) and the paths `docs/gateway.md` lists as refused by name (`text.format`, `output_config.format`, `thinking.display`, `context_management`). `docs/gateway.md` "The request".

## 0.20.0 (2026-10-30)

### Added

- Each `translation-unsupported` or `translation-invalid` refusal writes one `info` line, `translation refused`, naming the model, the client's wire, the code and llm's diagnostic, which names the refused path or field and never a value the client sent; the answer is unchanged. `docs/gateway.md` "The request".

### Changed

- **Breaking:** a trusted-file refusal no longer quotes the path it was given or the file a link resolves to, so a secret written into `--config` or a `*_file` setting by mistake is not printed or logged. The message is `<setting>: <detail>` (`owner_secret_file`, `routes`, `models.<alias>.vllm_api_key_file`, `hosted.<alias>.credential_file`, `routes.<endpoint>.auth.secret`, `metrics_bearer_file`, `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`; the document's own is `<detail>`), and the Bedrock container token's fetch warning carries `detail` in place of `file`. `credential set`, which writes the same `*_file` settings, does the same in its `credential:directory-missing`, `credential:directory-unsafe` and `credential:write` refusals (`<setting>: <detail>`), and its success line is now `credential stored <setting> probe <passed|skipped|not-rejected> http <code|->` in place of `credential stored <path> …`. Refusal codes are unchanged.
- **Breaking:** `ConnectorsRunpod` runs the command lines connectors 0.60.0 keeps: `invoke runpod/<adapter> <operation>` with `--connection`, the `--schema` and `--revision` pins and `--input-json`, and `--connector runpod --instance <adapter>` on every other command; it needs connectors 0.60.0 or newer. The `[providers.<name>.connectors]` key `adapter` is unchanged and names the `runpod` instance.

## 0.19.0 (2026-10-30)

### Changed

- llm 0.19.0 (was 0.18.0) for every llm client crate.
- A translated Messages request with a `system` entry inside `messages`, as headless Claude Code sends after the user prompt, reaches every translated route and Bedrock instead of `translation-unsupported`. Leading entries join the system prompt in order; a later one is user text in the mark `<system-message>\n<text>\n</system-message>` at its place, after the tool results when it follows a tool call. `docs/gateway.md` "Translation".

## 0.18.0 (2026-10-30)

### Added

- An OpenCode Go endpoint is sent the caller's own valid `x-opencode-session` (1 to 256 bytes of printable ASCII without a space, checked as sent), else `llmgw-` and a hash of the caller's address under a per-process key, else the declared static one, on the byte-relayed and the translated path, so each client keeps its own upstream session. `docs/gateway.md` "The OpenCode Go session".
- `llm_gateway::Caller`, `CALLER_SESSION_HEADER` and `RelayTarget::caller_header` (default `None`): a target may send a per-caller header in place of a static header of the same name.

### Changed

- **Breaking:** a static `x-opencode-session` declared for an OpenCode Go endpoint is sent only when the caller sent no valid session and its address is unknown.
- **Breaking:** `TranslationRequest` has a `caller` field, the request's `Caller`; an embedding that builds one sets it.

## 0.17.0 (2026-10-30)

### Added

- Conformance scenarios for the metrics-only listener (`metrics_listen`: `GET` and `HEAD` on exactly `/metrics`, `path-unknown` for every other path or method, its scrape token) and for the first sealed reasoning item's `id` and summary; the conformance runner can bind that listener and send it a scrape.

### Changed

- **Breaking:** a `metrics_bearer_file` token under 32 bytes, the owner secret's floor, is refused at start as `metrics-bearer:not-a-token`, naming `metrics_bearer_file`.
- **Breaking:** on the byte relay, a non-streamed client that shuts its sending side while the gateway waits for the answer head counts as gone, as it already did on the translated path; a client that half-closes must keep its sending side open until the answer begins.
- **Breaking:** `RelayStream` is no longer implemented for every `Read + Write + Send` type: an embedding implements it for each stream type it opens (`TcpStream`'s is provided), and `RelayStream::shutdown_handle` names the socket the relay shuts down when the client leaves before the answer head (`None` by default).

### Fixed

- A probe (`/health`, `/ready`) or a public route (`GET /v1/models`, `GET /`) is answered when its head repeats a header it never reads, such as `authorization` or `x-probe`, instead of being refused `request-malformed` or `credential-malformed`.
- A request head holding a bare LF, a stray CR or a NUL is refused `request-malformed` as soon as that byte arrives, so one past `request-head-bytes` is no longer `request-too-large` and one under it no longer waits for `read_timeout`.
- A Runpod pool step that comes later than `lease_ms` no longer terminates a pod with a request in flight: the stop the lease expiry owes waits until the last stream ends, and the first step after that submits it. A pod past its lifetime ceiling is still stopped with a stream open.
- `docs/hosting.md` describes the compute authorization the gateway passes: ledger `b10x-llm-gateway` with reservation `unmetered` or `spend-ceiling`, checked by ledger name only, with the Runpod spend check as the only spend cap. It no longer says a create needs an `llm-cost` reservation reserved and begun first.
- A deployment-document refusal no longer quotes the refused value: a string outside a closed set (`auth_kind`, `ingress_auth`, …) or where a number or a list belongs is refused `config:schema` naming its line and the values allowed, never the string written.
- The serving command line no longer echoes an unexpected argument: a usage error is `b10x-llm-gateway: usage error (<kind>)` with the usage line, exit status 2.
- With `ingress_auth = "none"`, the warning that ingress authentication is off is written before the listener is bound, not after it already serves.
- A byte-relayed call whose client closes its connection while the gateway waits for the answer head releases the target within about 100 ms and shuts its connection to the target down, a plain `http` one and a TLS one such as a Runpod pod's alike. A client that pipelined a next request before it closed counts as gone too: the bytes after its request are read and discarded, at most 64 KiB a look, since the answer carries `connection: close`.
- A target's chunked answer whose chunk extension carries a non-UTF-8 byte (obs-text in a quoted value, RFC 9112 section 7.1.1) is relayed and the target kept; only the chunk size is read, and one that is not `1*HEXDIG` is still refused.
- The conformance runner's request step keeps the client's sending side open unless it sets `half_close: true`, so relay scenarios no longer pass only by answering inside the gateway's 100 ms client watch.
- A translated call refused after its target answered records that status in its usage record, and on a route its model declares counts one `llmgw_route_upstream_status_failures_total`; `TranslatedAnswer::target_status` is the port's new method, and the binary reads the status from llm's response tap.
- A target answer with a non-UTF-8 byte in a header or trailer field the gateway does not read is relayed and the target kept; the status line and the `content-type`, `content-length` and `transfer-encoding` values must still be UTF-8.
- With a translator, route series are registered only for the (model, wire) pairs a model declares; a translated request on another wire feeds the process-wide series only, as `docs/gateway.md` states.
- `credential set` no longer quotes an unknown `--model` or `--route` name in `credential:unknown-target`: a generated key has the shape of a plain name, so the refusal calls it "the given name" whatever it is.

## 0.16.0 (2026-10-30)

### Added

- Manifests in `deploy/kubernetes/credential-namespace/` and a guide, "Run in a credential namespace": the gateway in the namespace `agent-credentials` with the provider key as a mounted file, `ingress_auth = "none"`, a Service and a NetworkPolicy that admits ingress from the namespace `agents` on port 8080 only.

### Changed

- ESS 0.77.0: CI installs it and the conformance runner builds on `ess-conformance` and `ess-primitives` 0.77.0; the suite and schemas it generates are unchanged.
- The llm crates move to tag `0.18.0`. The translator sets no `Controls::return_unpublished_calls`, so a call to an unpublished tool is still refused.
- Crates: `base64` 0.23, `clap` 4.6, `tokio` 1.54, `webpki-roots` 1.0.11, `sha2` 0.11.1, and `Cargo.lock` at the newest compatible versions.
- The site builds on `@beyond10x/docs-system` 0.14.1 and React 19.3. CI's gate checks out with `actions/checkout` v7.0.1 and uploads with `actions/upload-artifact` v7.0.2; the release logs in with `docker/login-action` v4.7.0.

## 0.15.0 (2026-10-26)

### Changed

- ESS 0.69.2: CI installs it, the conformance runner builds on `ess-conformance` and `ess-primitives` 0.69.2, and the suite and schemas are regenerated with it.
- The generated JSON Schema of `Seal` anchors its pattern and admits only the seal alphabet after the prefix.
- The llm crates move to tag `0.15.0`; a routes document naming an `aws-process` Bedrock endpoint (llm 0.14.0) is refused `config:value`.
- A translated request to an OpenCode Go messages endpoint carries the `x-opencode-session` llm 0.15.0 mints, new per request; a relayed one carries none.

## 0.14.0 (2026-10-23)

### Added

- A Bedrock `anthropic-invoke` model may declare `anthropic_beta = [..]`; only those client beta names reach its body.
- Messages client, InvokeModel model, non-empty list: an unlisted beta is logged by name, a malformed header `translation-invalid`.

### Changed

- The llm crates move to tag `0.13.0`; `http` 1.5 is added to hand llm the client's `anthropic-beta` lines.
- A Bedrock turn is checked with llm's `llm_bedrock::check_request`; the gateway's copy of the Converse rules is removed.
- **Breaking:** `TranslationRequest::anthropic_beta` is the client's one `anthropic-beta` line, `Option<&str>`, not a flag.
- A routes document naming a Bedrock endpoint, a route `aliases` or a route `price` (llm 0.12.0 keys) is refused `config:value`.

## 0.13.0 (2026-10-23)

- Amazon Bedrock models: a `[hosted.<alias>]` table with `kind = "bedrock"` declares `region`,
  `model_id` (a model id, inference-profile id or ARN) and `credential_source = "container"`,
  and optionally `api` (`anthropic-invoke`, the default for an Anthropic id, or `converse`),
  `endpoint`, `billing_kind`, `context_window` and `tool_calling` (default `parsed`). Every
  request to it is translated and sent with llm's `llm-bedrock` client, signed with AWS
  Signature Version 4 from the container credentials endpoint
  (`AWS_CONTAINER_CREDENTIALS_FULL_URI`, and `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE` read
  through the trusted-file reader at start and on each fetch, its symbolic links resolved so a
  projected-volume token and its rotation work). A field Bedrock does not take
  (a reasoning effort; parallel tool calls on `converse`; thinking on a model that is not
  Anthropic's) is dropped and its name logged, never its value. The inventory lists the model
  as `<alias>.bedrock` with `auth_kind` `aws-sigv4`, never its region or model id. New startup
  refusals: `container-credentials:unset`, `container-credentials:uri` and
  `container-token:<rule>`. Tested against a fixture Bedrock Runtime only.
- A turn to a Bedrock `converse` model that Converse cannot carry (an assistant message first,
  tool history without `tools`) is refused `translation-unsupported` (400) before anything is
  sent, instead of `upstream-refused` (502).
- A Bedrock model may declare `max_output_tokens` (1 to its window); it bounds the default output and is listed.
- An `anthropic-beta` header sent to a Bedrock model is logged by name as dropped; it never reaches Bedrock.
- A container token file a credentials fetch refuses is logged at `warn` with its rule code and path, never the token.
- A container token link resolving outside the token path's directory is refused `container-token:symlink`.
- The container token file is read on the runtime's blocking pool, so a slow mount holds no worker thread.
- The llm crates move to tag `0.11.0` under their `llm-*` package names, and `llm-bedrock` is
  added; still no AWS SDK crate and no `aws-lc` in `Cargo.lock`.

## 0.12.0 (2026-10-17)

- The translator runs on llm 0.7.0 (`b10x-llm-gateway-cli`'s llm crates and `b10x-llm-runpod`'s
  `b10x-llm-providers` at tag `0.7.0`). On a translated route, Claude Code's first turn
  (adaptive `thinking`, a `system` of text blocks, `cache_control`, effort `max`) and Codex's
  first turn (`instructions`, item ids, `parallel_tool_calls`, `prompt_cache_key`,
  `text.verbosity`, `reasoning.summary`, `client_metadata`) are carried to another wire instead
  of refused: effort `max` reaches a chat or responses upstream as `xhigh`, disabled thinking as
  `none`, and a hint the upstream wire has no place for is not sent. A key llm does not name is
  still refused at every level, now by llm itself. Still refused `translation-unsupported`:
  structured output, `context_management`, `thinking.display`, hosted tools, a thinking budget
  to a chat or responses upstream, and a mid-conversation instruction. Claude Code's later
  turns, which replay reasoning another upstream minted, are still `translation-opaque-state`.
  The refusal codes are unchanged.
- Codex's later turns are served on a translated route. A translated answer to a Responses
  client carries the upstream's reasoning state as a `reasoning` item whose `encrypted_content`
  is a seal (`llmgw-seal-1.`): the state and an HMAC-SHA256 tag over the route and both wires,
  under a key the gateway generates when it starts and never writes out. The next turn on the
  same route binds it: a Messages upstream gets back the signed `thinking` block it minted, and a
  Chat upstream gets the turn without the item. An item the gateway did not seal for that route
  since it last started (another route or wire, altered, forged, an upstream's own, or sealed
  before a restart) is still `translation-opaque-state`, whose message is now "the request
  carries reasoning state the gateway did not seal for this route". No seal is handed to a
  Messages client, so Claude Code's later turns stay refused. No log line, event, metric or
  refusal carries a seal or the state in it.
- `GET /metrics` also serves, per model, a latency histogram
  `llmgw_request_duration_seconds{model}` with fixed buckets from 0.05 to 300 seconds, and an
  outcome counter `llmgw_requests_total{model,outcome}`, `outcome` being `ok`, `upstream_error`,
  `timeout` or `refused`. Every model is registered at zero; labels are model aliases and fixed
  words only. `docs/gateway.md` lists the buckets and maps every refusal code to an outcome.
- `metrics_listen` in the deployment document binds a second listener that answers
  `GET /metrics` without the owner secret and refuses every other method and path with
  `404 path-unknown`. `metrics_bearer_file`, read by the credential-file rules
  (`metrics-bearer:<rule>` refusals), makes a scrape send that token; without it the listener is
  open. `metrics_listen` must differ from `listen`, and the owner-guarded `/metrics` is
  unchanged. The guides Scrape the counters and Run on Kubernetes (scrape port and network
  policy) describe it.

## 0.11.0 (2026-10-17)

- `b10x-llm-gateway credential set --config <file> --model <alias>` (or `--route <name>` for a
  route of the routes document) stores one upstream key in the file the deployment already names
  for it, the documented way to store one. The key comes from a hidden prompt on the terminal or
  from standard input with `--stdin`, never from an argument or the environment, and is held to
  the reader's token rule. Unless `--no-probe`, it is checked first with one
  `GET <base path>/models` (`/v1/models` when the base URL has no path) carrying the entry's own
  credential header and static headers over the gateway's TLS client, within 30 seconds overall
  and past any interim `1xx` answer: `401` or `403`, or a probe that cannot connect or does not
  answer in time, stores nothing. A command-line error prints one fixed usage line and never an
  argument's value. The file
  is written exactly as the key, mode `0600`, in a directory only you or root can write, read
  back through the server's own reader and only then renamed over the old one. Refusals are
  `credential:<rule>` codes, listed in the refusal reference; no line carries the key.
  `b10x-llm-gateway --config <file>` serves exactly as before. A guide, Store an upstream key,
  and the deployment document reference's rules for a credential file describe it.

## 0.10.0 (2026-10-17)

- The owner can wake a model ahead of demand and read its phase without a model call:
  `POST /v1/models/<alias>/wake` starts a Runpod model's pod as a request would, through the same
  spend check and profile fallback, and answers `202` at once with its status; while the pod
  starts or serves it starts nothing more. `GET /v1/models/<alias>/status` answers `stopped`,
  `starting` (with the provider's sub-phase), `ready` or `stopping` and the profile, read from
  the pool's records with no provider call. Both take the owner credential; a hosted model or a
  route has neither and is answered `path-unknown`. A model of a provider with no `connectors`
  connection is `target-unavailable` to a wake and `stopped` to a status, beside a connected
  provider too, and a sub-phase or profile reported empty is left out of the answer.
- A read of a target that an interrupt (a signal, a stop and continue) cuts short keeps the read
  timeout it began with, on a plain connection and on a pod's TLS connection: it is answered
  `upstream-failed` when that timeout is spent, however often it is interrupted.
- Every release tag now publishes the gateway image `ghcr.io/beyond10x/llm-gateway:<version>`
  for `linux/amd64` and `linux/arm64`, each built natively from the root `Dockerfile` and labelled
  with the tag's commit, through the `Release image` workflow (`.github/workflows/release.yml`).
  It never replaces a version the registry already holds, and a registry answer other than not
  found, or a missing docker, stops the release instead of passing it. The package is private when
  first created.
- A guide, Run on Kubernetes, runs the released image with the file rules unchanged: Secrets copied by an init container into an `emptyDir` the `nonroot` user owns at `0400`, the refusal an `fsGroup` causes (`owner-secret:unsafe-mode`, pasted from a run and asserted by a test), probes, and a `preStop` sleep with a `terminationGracePeriodSeconds` that covers the longest answer.
- A model may declare ordered `[[models.<alias>.profiles]]`, each a GPU type list with its vLLM
  settings. When Runpod refuses every create of a profile for capacity (the connectors failure
  code `capacity`), the start moves on to the next profile, inside the same startup deadline and
  with the spend check before every create; any other refusal ends the start. Two profiles sharing
  a name or a GPU type are refused `profile-conflict`. The flat declaration is the one profile
  `default`, unchanged, and the usage record names the profile of the pod that served.
- A messages request served by a fallback profile is clamped to that profile's `max_model_len`,
  and `GET /v1/models` lists the smallest window among a model's profiles. A start sends no
  further create, of a profile's next GPU type or of the next profile, once its startup deadline
  has passed, and ends `no-capacity`.
- A provider may declare `daily_cap_usd` and `total_cap_usd`, spend ceilings for its account,
  and each model `price_per_hour_usd`. Before every pod create the pool checks the account's
  reported spend plus its live pods' and the new pod's estimates against each ceiling; a start
  that would pass one, or whose spend is unknown, is refused `spend-ceiling` (503) with no
  create, counted in `llmgw_pod_start_spend_refusals_total`. The reported spend is read through
  the connectors operation `billing.pods`, which needs a connectors release that offers it.
- The shipped gateway enforces the spend ceilings: its Runpod pool is built with them, as every
  other composition's is. A live pod counts at least its startup estimate, so two starts in a
  row do not share one headroom; a reported amount in whole cents is read exactly; a
  `billing.pods` result with no status refuses the start.
- A spend ceiling binds only the account of the provider that declares it: a ceiling declared on
  a provider other than the connected one no longer refuses the connected provider's pod starts.

## 0.9.0 (2026-10-16)

- A hosted model's `base_url` may carry a path. The path replaces the wire path's leading `/v1`,
  so `https://openrouter.ai/api/v1` sends Chat requests to `/api/v1/chat/completions` and a
  `base_url` ending in `/v1` works as OpenAI-compatible clients write it; a `base_url` without a
  path reaches the wire path as before. The output clamp's count follows the same rule, and
  `host` stays the authority. A base path with an empty, `.` or `..` segment, a `%`, a query, a
  fragment or user information is refused `config:value`.
- A hosted model may declare `[hosted.<alias>.headers]`, static non-secret request headers sent
  with every request to its endpoint (Anthropic's `anthropic-version`, OpenRouter's
  `HTTP-Referer` and `X-Title`), lowercase and in name order after `host`. A header the gateway
  writes or that frames a message, `authorization`, the endpoint's own `credential_header`, a
  repeated name, more than 32 headers, and a value that is empty, over 1024 bytes or not
  printable ASCII are refused `config:value`. Client headers are still never forwarded.
- Ingress authentication for model calls is optional: `ingress_auth = "none"` serves the three
  wire paths without a credential, for a gateway behind something that authenticates already,
  and the binary writes `warning ingress auth is off` before `listening on`. `owner_secret_file`
  is then optional; `/metrics`, `/v1/routes` and unknown paths still take the owner secret, and
  without one they refuse every request. `bearer`, the default, is unchanged, except that a
  document without `owner_secret_file` is now refused `config:value` rather than
  `config:schema`. The library gains `GatewayConfig::ingress_auth` (`IngressAuth`) and
  `NoOwnerVerifier`.
- The gateway translates between wires within llm 0.6.0's published subset. A request on a wire
  the model's upstream does not declare is decoded, checked against the model's capabilities and
  projected before any target is asked for, sent with llm's client for the model's first declared
  wire, and answered on the client's wire, streamed event by event with the upstream's usage, or
  as its JSON object. Same-wire requests are relayed byte for byte as before. New refusals:
  `translation-unsupported`, `translation-invalid`, `translation-opaque-state` (400),
  `translation-unrepresentable` and `upstream-refused` (502). `GET /v1/models` lists every wire
  and the model's `upstream_wires`; `GET /` profiles stay on the declared wires, because Claude
  Code and Codex send fields the subset refuses. The library gains the `Translator` port,
  `Relay::with_translator`, `RelayTarget::scheme` and `TargetBearer::expose`; the binary carries
  llm's client crates (by tag) and a tokio runtime, and always composes `LlmTranslator`.
  A key below the top level of a chat or responses request that the subset does not list is
  refused, not dropped; before output a transport failure (unreachable, timed out, a reset or
  cut connection) is `upstream-failed` and replaces the target, while an answer that broke its
  wire is `upstream-refused`; an answer past a published bound of the translated path (65536
  events, 32 MiB, a 1 MiB line or event, 64 KiB of one tool call's arguments) is
  `translation-too-large` (502) or ends with that terminal error; a messages upstream asked by a
  request naming no output limit gets `min(16384, w / 2)` of the window, 200000 by default; and
  a non-streamed client that closes cancels the turn, recorded `Relayed` with status 0.
- The deployment document may name an llm `llm.routes/1` document, `routes = "<path>"`, read
  once at start (at most 2 MiB, not group- or world-writable) and parsed with llm 0.6.0's
  `Routes::parse` (`b10x-llm-models`, no default feature, so llm reads no secret). Each route is
  served under its name from a hosted endpoint built from its endpoint: relayed byte for byte on
  the endpoint's wire to its base path, translated from the other wires under the route's
  capabilities, beside `[models.*]` and `[hosted.*]`. `GET /v1/models` lists each route on all
  three wires; `GET /v1/routes` lists one target per route, provider `routes`, the endpoint's
  name, and no URL or credential. A credential comes only from a `file:` key file read through
  the trusted reader (`endpoint-credential:<rule>`, `~/` against `HOME`); an `env:` or
  `keychain:` secret, a `base_url` without its API path or outside the base-path rule, a
  cleartext credential to a remote host, a document llm refuses, and a route name equal to a
  `[models.*]` or `[hosted.*]` alias are refused `config:value`, and so is a route name over
  246 bytes, the longest whose target id `<route>.<wire>` fits an inventory label. A route on a
  `messages` endpoint is sent llm's `anthropic-version` on every request, the output clamp's
  count included, and no other static header. A document with only `routes` is admitted. The
  library exports `Routes`, `Route`, `RouteEndpoint`, `SecretFile`, `MAX_ROUTE_NAME_BYTES` and
  `Deployment::with_routes`; the conformance `Relay` program takes `routes_document` and
  `route_credentials`.

## 0.8.0 (2026-10-16)

- Claude Code on the Messages wire: an effort of `max` (`output_config.effort` or
  `chat_template_kwargs.reasoning_effort`) now reaches the model as `xhigh`, as `high` already
  did, because the served Qwen3.8 chat template accepts only `xhigh`, `medium` and `low`.
  Adaptive `thinking`, `output_config.format` and the deprecated `output_format` are relayed
  unchanged, `anthropic-version` and `anthropic-beta` are dropped like every client header, and
  `HEAD /api/hello` stays refused; `docs/gateway.md` "The relay" says why for each, from vLLM
  v0.27.1's source and Claude Code's published gateway protocol. Settled from those sources and
  authored fixtures, not from a live session (`docs/verification/2026-10-15-claude-code-wire.md`).
- A hosted model may declare `reasoning_effort = "low" | "medium" | "xhigh"`. A Messages request
  that names no effort reaches its endpoint with `output_config.effort` set to it, so a server
  started by hand no longer thinks at its chat template's `xhigh` default when Claude Code sends
  no effort. A request that names one keeps it; the Chat and Responses wires and Runpod models,
  whose effort is the pod's own default, are unchanged.
- A Messages request never asks for more output than the model's window has room for. For a
  model with a window (a Runpod model's `max_model_len`, a hosted model's `context_window`), a
  request with a top-level integer `max_tokens` is first counted at the same target's
  `POST /v1/messages/count_tokens`; a `max_tokens` above the room the input leaves is lowered
  to it, and an input that leaves none is refused `context-exceeded` (400), whose message starts
  `prompt is too long` so Claude Code compacts instead of meeting vLLM's 500. A count that fails,
  or is not answered within `GatewayConfig::count_timeout` (ten seconds by default) of its
  connection opening, relays the request unchanged; a hosted model without a `context_window`, and the Chat and
  Responses wires, are not counted.
- The usage block of a model's answer, cache details included, is now held to reach the client
  unchanged on the Chat and Messages wires, streamed or not: a vLLM started with
  `--enable-prompt-tokens-details` reports `usage.prompt_tokens_details.cached_tokens` on the
  Chat wire and `cache_read_input_tokens` and `cache_creation_input_tokens` on the Messages wire,
  and a usage field the gateway does not know passes too. No behaviour changed: the relay already
  returned the answer's bytes unchanged; tests and scenarios now pin it, against answers authored
  from vLLM v0.27.1's source. The usage record's token counters stay absent.

## 0.7.0 (2026-10-14)

- The documentation site is published at <https://beyond10x.github.io/llm-gateway/>; README
  links it instead of the pages in the tree.
- Pod proxy TLS: the binary reaches a Runpod pod at `https://<pod id>-8000.proxy.runpod.net/v1/`
  over TLS (`rustls` on `ring`, in `b10x-llm-gateway-cli` only), the readiness probe and every
  relayed request alike, with the certificate verified against the Mozilla roots `webpki-roots`
  compiles in before any request byte is written. A certificate that does not verify is
  `upstream-failed`. No document key or option adds a root or disables verification. The
  start-time refusal of a document that declares a `connectors` provider is gone, so the binary
  now starts and relays to pods. `b10x-llm-runpod`'s probe takes its TLS connection from the
  embedding (`ConnectorsRunpod::set_tls`, `PodTls`). A binary started with a `connectors` provider
  and real Runpod credentials starts real, billed pods; no test reaches Runpod.
- `spec/domains/hosting.yaml` and `spec/domains/runpod.yaml` declare every rule the hosting
  contract promises: the hosting refusals with their codes and messages, the transition table,
  the evidence routes and listing completeness as typed enums, and the Runpod pool and
  declaration refusals with their codes. `b10x-llm-provision` publishes the evidence routes as
  `EvidenceKind`.
- A pod connection's 10 s connect bound covers the TLS handshake as one deadline; the handshake
  no longer waits under the 120 s read timeout. The `gateway-tls-seam` test program is built only
  with the test-only `test-seam` feature, so the release build writes `b10x-llm-gateway` and no
  program that trusts a root from its environment.
- Hosted endpoints: a model declared as `[hosted.<alias>]` is relayed to any server that speaks
  its wires at an `http` or `https` base URL (`base_url`, no path), as `bearer`, as `api-key` in
  the header its `credential_header` names, or `anonymous`, with the credential read once from
  `credential_file` through the trusted-file reader (`endpoint-credential:<rule>`). `https` uses
  the pod connection's TLS client and roots. A `bearer` or `api-key` endpoint at an `http` base
  URL whose host is not loopback (`127.0.0.0/8`, `::1`, `localhost`) is refused as
  `config:value`, so no credential is sent in cleartext to a remote host. The route inventory
  lists each hosted target's
  `auth_kind` and `billing_kind`, never its URL or credential. The gateway library gains
  `CredentialHeader` and `RelayTarget::credential_header` (`credential-header-bytes`, 64).

## 0.6.0 (2026-10-08)

- `GET /v1/models` and `GET /` answer without a credential and wake no pod. The listing is the
  OpenAI list shape, one entry per model with its `max_model_len` and wires. `GET /` answers the
  Codex, Claude Code or Loom settings for each model that declares the client's wire and parses
  tool calls, chosen by `User-Agent`, and counts one `llmgw_instruction_views_total`.
- A request head that is not UTF-8 is `request-malformed` on every route.
- `spec/domains/gateway.yaml` and `spec/domains/deployment.yaml` declare every rule the gateway
  contract and the README promise; the conformance suite holds 201 scenarios (was 191).
- A documentation site under `website/`, generated in part by the new `llm-gateway-docs` crate
  (the CLI, crate and refusal references and the status page), built by the `Documentation
  validation` workflow and deployed by the `Documentation site` workflow. `task check` and
  CI's `Gate` run `llm-gateway-docs generate --check`.
- The binary's clap definition moved to `llm_gateway_cli::cli::Cli`, so the CLI reference is
  generated from it. The command line is unchanged.

## 0.5.0 (2026-10-08)

- Runpod production transport: `ConnectorsRunpod` reaches Runpod only through the `connectors`
  CLI and its Runpod bundle (connectors v0.37.0): `pod.create`, `pods.list`, `pod.terminate`,
  each write with an approval proof for its exact input. An `unknown` create is resolved by one
  listing on the pod's name and request tag; a stale descriptor or an expired connection proof
  gets one bounded recovery.
- Model tool calling: each model declares `tool_calling = "parsed" | "absent"` (default
  `absent`). A non-empty `tools` array for an `absent` model is refused `tools-not-served` (400)
  before any target is asked for, so it starts no pod.
- Live Runpod wiring: `start_connected` composes the pool over `ConnectorsRunpod`. The shipped
  binary refuses a deployment document that declares a `connectors` provider until it can reach
  the pod's `https` endpoint, so this release cannot create a pod.
- Conformance: 191 scenarios (was 187).

## 0.4.0 (2026-10-08)

- Cold-start hold: a request for a model whose pod is starting is held up to the model's new
  `request_hold_seconds` (default `start_wait_seconds`), bound to that pod; past the budget it is
  answered `model-cold-start` (503, `retry-after: 30`). A pod that fails during the hold, or a
  hold that passes unbound, is `target-unavailable` for every request held on it. One cleanup
  pass before serving, then every 60 s.
- `RelayTargets::acquire` returns `Result<_, TargetRefusal>`; `b10x-llm-runpod` gains `Hold` and
  `RunpodPool::ensure_held`.
- Observability: `GET /metrics`, behind the owner credential, serves llmgw's 13 counters under
  their `llmgw_` names; one usage record per authenticated wire request; the binary logs through
  `tracing` with its level from `RUST_LOG`.
- Conformance: 187 scenarios (was 181).

## 0.3.0 (2026-10-08)

- ESS 0.56.0: CI installs it, the conformance runner builds on `ess-conformance` and
  `ess-primitives` 0.56.0, and the suite and schemas are regenerated with it.
- The relay sends each Runpod pod its model's vLLM key as `authorization: Bearer <key>`, and
  nothing of the client's request head. A failed pod is stopped and replaced, the graceful stop
  is never held by a request waiting for a pod, and `idle_timeout_minutes = 0` stops a pod at the
  first pass with no request in flight.
- A root `Dockerfile` builds a distroless, non-root image of `b10x-llm-gateway` on port 8080 with
  a source revision label. `docs/model-profiles.md` holds two proven model declarations and the
  weight-cache block.
- Conformance: 181 scenarios (was 179).

## 0.2.0 (2026-10-08)

- The deployment document takes an optional `vllm_api_key_file` per model, read once at startup
  through the trusted-file reader and held redacted; a broken file refuses with
  `vllm-api-key:<rule>`. `runpod_api_key_file` is refused: connectors holds the Runpod API key.
- `b10x-llm-runpod` takes the pod address from llm 0.5.0's Runpod provider description
  (`https://<pod id>-8000.proxy.runpod.net/v1/`). A pod whose id the description refuses gets no
  address and is never ready.

## 0.1.0 (2026-10-08)

- First release: the single-owner gateway library and the `b10x-llm-gateway` binary with the
  wire relay, the hosting lifecycle contract, the Runpod adapter over the in-process emulator,
  the ESS specification and its conformance suite (179 scenarios), and the design for using a
  Runpod-hosted model from Claude Code, Codex and Loom (`docs/design/runpod-clients.md`).
