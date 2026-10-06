# Design review: hosting, 2026-10-06

This is the findings table of the `ess:hardening` design review (technique 8), recorded in
[spec-hardening.md](spec-hardening.md). One agent compared `docs/hosting.md` with
`spec/domains/hosting.yaml` and `spec/domains/runpod.yaml`. Every line number refers to
llm-gateway `a11ef83`. The reviewed specification was a copy with one planted deletion:
`Disowned` was removed from `llm-gateway.hosting.Phase`.

The rows are quoted as the reviewer returned them. The status column is this record's own:

- `plant`: the planted deletion, caught.
- `fixed`: corrected in the hardening change.
- `open`: owned by `story:hosting-spec-declarations`.

| # | status | classification | design (file:line, quoted) | spec (file:line, quoted) | note |
|---|---|---|---|---|---|
| 1 | plant | contradicts | hosting.md:77 "`Disowned` is a resource a strictly newer epoch has taken over"; :85 "Stopped, Cancelled, Disowned are terminal" | hosting.yaml:13 "variants: [Declared, Requested, Active, Uncertain, StopRequired, Stopped, Cancelled]" | The planted deletion. |
| 2 | fixed | contradicts | hosting.md:274 "The ESS domain `llm.hosting` describes adapter observations" | hosting.yaml:1 "domain: llm-gateway.hosting" | The document now names `llm-gateway.hosting`. |
| 3 | fixed | contradicts | hosting.md:34 "The same split runs through `spec/domains/catalog.yaml`, where `llm.catalog.DeploymentSpec`" | hosting.yaml:84-86 "Both entities belong to the system `llm` …" | The document now names llm's `catalog.yaml`. |
| 4 | fixed | stale mapping | hosting.md:74 "`Phase` and `TRANSITIONS` in `machine.rs` are the whole ownership state machine." | hosting.yaml:144 "resolves the formerly open hosting transitions (docs/design.md:82-83)" | `docs/design.md` does not exist here. The pointer now names "The resolved lifecycle" in `docs/hosting.md`. |
| 5 | open | missing | hosting.md:5 "opens no socket, reads no credential, holds no cloud SDK and allocates no resource" | hosting.yaml:150-151 "drives the real controller against the real in-process fake provider" | runpod.yaml:7-9 says this for Runpod; hosting.yaml does not. |
| 6 | open | missing | hosting.md:22-23 "same name and a different incarnation is simply not found. It is not adopted, not counted and never stopped." | hosting.yaml:102-106 "`resource_incarnation` is what distinguishes them" | The identity rule is declared; its three consequences are not. |
| 7 | open | missing | hosting.md:30-32 "it never means `false` … `is_ready` is `ready == Some(true)`" | hosting.yaml:63 "observed_ready, type: 'Optional<Boolean>'" | |
| 8 | open | missing | hosting.md:40-42 "Disconnecting a client is not shutdown … Neither is releasing a lease, restarting the controller, or reading a listing that did not claim to be complete." | hosting.yaml:49 "{name: connected, type: Boolean}" | |
| 9 | open | missing | hosting.md:48 "every one of them attaches evidence"; :52-55 the four evidence routes; :58-59 "no evidence is attached" | hosting.yaml:101 "stop_evidence, type: 'Optional<String>'" | No evidence vocabulary. |
| 10 | open | missing | hosting.md:66-69 "absence only from a listing that `Inventory::answers_request_identity` accepts …" | hosting.yaml:97-98 | `Completeness` is not declared. |
| 11 | open | missing | hosting.md:90, :92, :94, the absent edges | hosting.yaml:142-145 "transitions … read from its published constant" | The table is only observed, never declared. |
| 12 | open | missing | hosting.md:109-111 "refuses with `wrong-phase` … A silent `Ok` is no longer expressible for `RequestStop` or `ConfirmStopped`" | hosting.yaml:126 "error_code, type: 'Optional<String>'" | No `HostingError` code is declared. |
| 13 | open | missing | hosting.md:113-114 "`StopRequired`, `Stopped` and `Disowned` are reachable from every phase that can hold a resource" | hosting.yaml:142-145 | |
| 14 | open | missing | hosting.md:124-126 "`Stop` refuses such a record outright with `foreign-resource`" | hosting.yaml:17 "OwnershipLost" | Only the reason is declared. |
| 15 | open | missing | hosting.md:136 "The reason that opened an obligation is kept — except that `ownership-lost` always wins."; :147-148 the one place the slot is cleared | hosting.yaml:100 "stop_reason" | |
| 16 | open | missing | hosting.md:153-154 "becomes `Uncertain` with reason `ambiguous-mutation`, and it is **never** retried blindly" | hosting.yaml:20 | |
| 17 | open | missing | hosting.md:157-159 "A retry is refused with `idempotency-unsupported` unless the provider answers `honours_idempotency_key`" | hosting.yaml:97-98 | |
| 18 | open | missing | hosting.md:163-165 "`Cancel` is permitted only from `Declared` …" | hosting.yaml:21 Dispatch variants | |
| 19 | open | missing | hosting.md:169-170 "one lease per deployment. A second owner is refused while a live claim exists"; :263 "one epoch is granted to one owner" | hosting.yaml:116-118 | The lease has no deployment field. |
| 20 | open | missing | hosting.md:172-175 "`Controller::fence` refuses … **before** the provider is called …" | hosting.yaml:118 | |
| 21 | open | missing | hosting.md:177-180 "A restart is not a discharge."; :182-191 the clock floor | hosting.yaml:150-151 "restarts" | |
| 22 | open | missing | hosting.md:196-200 "refused with `foreign-resource` … the record keeps no observation and its obligation stays open" | hosting.yaml:73-74 | |
| 23 | open | missing | hosting.md:204-206 "`max_active`, 1..=4096 … never zero … refused … before any mutation is submitted" | hosting.yaml:76-77 | |
| 24 | open | missing | hosting.md:207-215, equal admitted and +1 ms refused; "**minimum**"; `lifetime-exceeded` obliges a stop | hosting.yaml:17; :54 | |
| 25 | open | missing | hosting.md:217-219 "refuses an authorization from a different ledger" | hosting.yaml:95-96 | |
| 26 | open | missing | hosting.md:225-226 "`inventory` and `HostingCommand::Validate` … must never allocate" | hosting.yaml:128-133 counters | |
| 27 | open | missing | hosting.md:295 incarnation is the Runpod pod id; owner, epoch and request labels read back from the listing | runpod.yaml:71-77 last_env | |
| 28 | open | missing | hosting.md:296 "a name starting with `llmgw-` is never listed, adopted, swept or terminated" | runpod.yaml:56-57 | |
| 29 | open | missing | hosting.md:297 an inherited pod is terminated only while its label names exactly the recorded holder | runpod.yaml:5-6 | |
| 30 | open | missing | hosting.md:299 "a **lost** create answer ends the attempt … `honours_idempotency_key` is `false`" | runpod.yaml:5; :53-54 | |
| 31 | open | missing | hosting.md:300 crash restarts counted inside `crash_window_ms`; a pod refusing its vLLM key is terminated | runpod.yaml:25 CrashLoop, CredentialRefused | |
| 32 | open | missing | hosting.md:301 "counted from the last time it was seen ready … unknown readiness neither starts nor ends that clock" | runpod.yaml:25 StartupDeadline, StoppedServing | |
| 33 | open | missing | hosting.md:302 "never below the measured cold start, never with a request in flight …" | runpod.yaml:61-62 cleanups | |
| 34 | open | missing | hosting.md:303 orphans carry this controller's owner tag and no live record holds them | runpod.yaml:61-62 "orphans=[..]" | |
| 35 | open | missing | hosting.md:304 the request-tag encoding: readable prefix of at most 64 bytes, at most 96 bytes, injective | runpod.yaml:72-74 | |
| 36 | open | missing | hosting.md:305 "only `TERMINATED` ends a resource … An endpoint is reported only for a `RUNNING` pod" | runpod.yaml:57; :65-67 | |
| 37 | open | missing | hosting.md:306 the vLLM key is a Runpod secret reference; no value passes through this process | runpod.yaml:29 SecretReference | |
| 38 | open | missing | hosting.md:310-311 "A create whose image differs from the declared one, or names an undeclared model, is not sent." | runpod.yaml:25 UnknownModel | |
| 39 | open | missing | hosting.md:313-314 "an expired lease is a stop obligation under this contract, and the pool carries it out" | hosting.yaml:17 LeaseExpired; :78 lease_ms | |
| 40 | open | unclear | hosting.md:300 "two decreasing uptimes" against :309 "crash-restart limit" | runpod.yaml:25 CrashLoop | Fixed at two, or configurable? `crates/llm-runpod/src/config.rs:81` has `crash_restart_limit`. |
| 41 | open | unclear | hosting.md:305 "it is reported present-but-not-running" | hosting.yaml:24 ProviderState | Which `ProviderState` an exited pod maps to. |
| 42 | open | unclear | hosting.md:301 (`stopped-serving`); :305 (`pod-exited`) | runpod.yaml:21,25 | Whether these are refusals a caller receives. |
| 43 | open | spec-only | hosting.md:196-200 says nothing on reset | hosting.yaml:80-81 "No reset or scope-transfer operation is exposed while obligations are outstanding." | |
| 44 | open | spec-only | hosting.md:44, :59 mention only `stop_required` and `transferred` | hosting.yaml:28-29, :36 active_count, unknown_count, at_capacity | |
| 45 | open | spec-only | hosting.md:298-299, nothing on a "stopping" answer or running out of GPUs | runpod.yaml:25 "Stopping", "NoCapacity" | |
| 46 | open | spec-only | hosting.md:308-311 list settings but no validation rules | runpod.yaml:29 ConfigRefusal variants | |

Counts: 46 rows. Without the plant: 2 contradicts and 1 stale mapping (all three fixed), 35
missing, 3 unclear, 4 spec-only.
