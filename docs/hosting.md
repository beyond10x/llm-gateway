# Owned-resource hosting

`b10x-llm-provision` is the lifecycle every hosting provider adapter is held to, plus
`FakeProvider`, an in-process provider that demonstrates it. The crate has no dependencies. It
opens no socket, reads no credential, holds no cloud SDK and allocates no resource; Runpod and
Modal are separate adapters behind this seam and are not implemented here. Nothing in the normal
gate makes a paid call.

Three rules explain most of the shape of the API, and each one exists because the obvious
alternative is wrong in a way that costs money.

## A name is not an identity

A provider may hand a resource name to a **later** resource once the previous one is gone. A
controller that matched its records on name would adopt that later resource, bill its own
reservation against it, and eventually stop somebody else's compute. `ResourceKey` is therefore
`(provider, account, name, incarnation)`, where `incarnation` is assigned by the provider at
creation and equality includes it. `ResourceKey::same_name` exists so a caller can ask the
narrower question deliberately; nothing in the contract uses it to decide ownership.

The consequences are in `Inventory`: `exact` matches the whole key, and a resource with the same
name and a different incarnation is simply not found. It is not adopted, not counted and never
stopped.

## Requested is not observed

`DeploymentSpec` is what an operator asked for. `ProvisionedDeployment` is what a provider
reported. The second can only be constructed from an `ObservedResource`, and no field of it is
ever filled in from the first. An unreported served model, address, runtime state or readiness is
`None`. `None` means the provider said nothing; it never means `false`, and it never silently
becomes the requested value. `ProvisionedDeployment::is_ready` is `ready == Some(true)`, so a
provider that stops reporting readiness stops being ready to every reader at once.

The same split runs through llm's `spec/domains/catalog.yaml` (beyond10x/llm), where
`llm.catalog.DeploymentSpec` carries the requested name and lifetime and
`llm.catalog.ProvisionedDeployment` carries the observed incarnation and four optional
observations.

## Nothing but evidence discharges a stop obligation

Disconnecting a client is not shutdown. An expired lease is not evidence that billing stopped.
Neither is releasing a lease, restarting the controller, or reading a listing that did not claim
to be complete. This is the same durable obligation `llm-cost`'s ledger models from the other
side of the seam, and the vocabulary is deliberately its: `Uncertain`, `StopRequired`, `Stopped`,
`Cancelled` and a `stop_required` list in the totals all mean here what
[the budget contract](budgets.md) says they mean there. A hosting controller's job is to produce
the evidence the ledger's `ConfirmStopped` needs, not to invent a second way of talking about it.

Four things close an obligation, and every one of them attaches evidence to the record:

| Route | Evidence |
| --- | --- |
| absent from a listing that can answer for this request | `inventory-complete-absent` |
| the provider reported the resource terminated | `provider-terminated` |
| an accepted stop mutation | the provider's own reference |
| `ConfirmStopped` with an external observation | the caller's reference |

A fifth thing *moves* an obligation without closing it. When a strictly newer epoch is observed
owning the resource, the record becomes `Disowned`: nothing stopped, so no evidence is attached,
and the deployment leaves `totals.stop_required` for `totals.transferred` rather than
disappearing out of both. An operator reconciling two controllers can then see which resources
the older one handed over.

The first route is narrower than it looks. Completeness promises that every resource in the
scope was listed; it promises nothing about whether each listed resource echoes the idempotency
key it was created with, and `ObservedResource::request_id` is optional precisely because
providers may not. So a record that has no resource key yet — an unresolved create — is settled
by absence only from a listing that `Inventory::answers_request_identity` accepts: complete
**and** with every listed resource carrying a request id. A listing that cannot answer the
question produces no transition at all, the same silence a partial listing produces. A record
that does have a key is settled by completeness alone, because a key is never optional.

## The resolved lifecycle

`Phase` and `TRANSITIONS` in `machine.rs` are the whole ownership state machine. `Phase` shares
`Uncertain`, `StopRequired`, `Stopped` and `Cancelled` with `llm.budget.Phase`; `Requested` is a
create the provider accepted but no listing has confirmed, `Active` is a resource observed under
this record's exact key, and `Disowned` is a resource a strictly newer epoch has taken over.

```
Declared    -> Requested | Uncertain | Cancelled
Requested   -> Requested | Active | StopRequired | Stopped | Disowned
Active      -> Active | StopRequired | Stopped | Disowned
Uncertain   -> Uncertain | Requested | Active | StopRequired | Stopped | Disowned
StopRequired-> StopRequired | Stopped | Disowned
Stopped, Cancelled, Disowned are terminal
```

Any pair outside that table is refused by `Phase::may_become`, and the refusals that matter are:

* `Uncertain -> Cancelled` and `StopRequired -> Cancelled` are absent. Cancellation releases an
  obligation that was never incurred; it cannot discharge one that was.
* `StopRequired -> Active` is absent. A later healthy observation does not withdraw a stop
  obligation.
* `Declared -> Active`, `-> StopRequired` and `-> Stopped` are absent. Nothing can be observed,
  obliged or discharged before a mutation was submitted.

The table is published as an observed fact, so a reader of the conformance suite sees the
transitions the library actually enforces rather than the ones a document claims.

**A phase write guarded on `may_become` and nothing else is silent when the table forbids it.**
That is not a hypothetical: code written for a foreign owner tag once asked for
`Active -> Uncertain`, which is not an edge, and the record stayed live and unobliged with no
diagnostic anywhere. `RequestStop` on a `Declared` record was the same defect in its other form
— `Declared -> StopRequired` is not an edge either, so the command answered `Ok` having recorded
no obligation at all, where `Cancel` refuses the mirror case with `wrong-phase`.

Three things stand against it now, and the third is what makes the first two more than a habit.

* `require_stop` and `discharge` **return whether the table let the write happen**, and a command
  whose only work is one phase change refuses with `wrong-phase` when it did not. A silent `Ok`
  is no longer expressible for `RequestStop` or `ConfirmStopped`; reconciliation still has nobody
  to tell, which is why it asks only for changes that are edges from every phase it can reach.
* `machine.rs` asserts that `StopRequired`, `Stopped` and `Disowned` are reachable from every
  phase that can hold a resource, so no obligation can become inexpressible.
* `machine.rs` carries the table of every phase change the controller asks for. The changes a
  *conditional* writer asks for stay a narrative list, marked permitted or a deliberate no-op, so
  a row that flips without being edited fails. The three writers **any** phase reaches —
  `require_stop`, `discharge` and `cancel` — are a grid instead, `Phase::ALL` against those three
  targets, and a test derives that grid from the vocabulary and requires a row per cell. A
  hand-written narrative cannot be checked for completeness, and this one was not complete: the
  missing cell was `Declared -> StopRequired`. A phase added to `Phase` now fails here until its
  three rows are decided.

A foreign owner tag that is not a strictly newer epoch calls `require_stop` with
`ownership-lost`, and `Stop` refuses such a record outright with `foreign-resource`: the provider
says that resource is somebody else's, and stopping it would be the exact failure the contract
exists to prevent.

### The one precedence rule on the reason slot

`Stop` reads `stop_reason` to decide whether the resource is still ours to touch, and several
writers share that slot, so the rule deciding who wins is part of the contract rather than an
implementation detail. It is stated once, in `record_stop_reason`, which is the only place the
slot is written:

> **The reason that opened an obligation is kept — except that `ownership-lost` always wins.**

The exception is not a special case for tidiness. Every other reason says why *this* controller
wants the resource stopped; `ownership-lost` says *whose the resource is*, which is a different
kind of fact and the one the refusal turns on. Without it, first-writer-wins quietly dropped the
foreign owner tag on both of the ordinary sequences that reach it — `RequestStop`, `Observe`,
`Stop`, and lease expiry, reacquisition by another controller, `Observe`, `Stop` — and the
controller destroyed another owner's compute and filed it as its own discharged obligation. Both
halves are asserted, over every ordered pair of published reasons, so a reason added to the
vocabulary is covered without a test being edited.

The slot is cleared in exactly one place: when the resource is seen again under this record's own
key with nobody else's label on it, which is the only observation that answers the question.

## Mutation ambiguity, idempotency and cancellation

`CreateOutcome::dispatch` uses `llm_core::Dispatch`'s four words. `Unknown` is the case the
contract exists for: the answer was lost, so a billed resource may or may not exist, the record
becomes `Uncertain` with reason `ambiguous-mutation`, and it is **never** retried blindly.

The idempotency key is `CreateRequest::request_id`, chosen and recorded before the mutation is
submitted so that a resource created by a request whose answer was lost can still be found. A
retry is refused with `idempotency-unsupported` unless the provider answers
`honours_idempotency_key` — an unsupported lifecycle action is reported, never simulated. The
only other recovery is reconciliation: `Observe` adopts a resource carrying that request id, and
a complete listing without one settles the record with evidence.

`Cancel` is permitted only from `Declared`. A create the provider refused or never sent is
withdrawn, because the provider answered that nothing exists; a create whose answer was lost is
not.

## Ownership fencing, and what restart does

`LeaseRegistry` grants one lease per deployment. A second owner is refused while a live claim
exists; an expired claim may be taken over. Every grant raises a strictly monotonic `epoch` that
is never reused, including when the original owner restarts — which is what fences a controller
against its own previous incarnation. `Controller::fence` refuses a mutation whose recorded epoch
is below the registry's current one **before** the provider is called, and a resource observed
carrying a strictly newer epoch moves the record to `Disowned`, after which this controller
issues no mutation against it at all.

`Controller::restore` is the restart. It keeps identity, phase and every open obligation, and it
drops two things deliberately: the lease, so a restarted controller must reacquire at a new epoch
before it may mutate anything, and every liveness observation, because a readiness seen before a
restart is stale rather than current. A restart is not a discharge.

It also refuses to reopen at an instant earlier than the snapshot already witnessed. `apply`
refuses a clock that moves backwards, but that guard lives on the instance, so a restart would
otherwise reset it — and a controller reopened earlier would un-expire leases and un-fire time
ceilings through the one door the guard cannot watch. The floor is the latest instant the
restored records witness.

That floor is weaker than "the snapshot is non-empty" makes it sound, and the narrower statement
is the true one: **a record witnesses an instant only if it carries a submission instant or an
observation, so a `Declared` or `Cancelled` record witnesses nothing and raises the floor by
nothing.** A snapshot made only of such records constrains nothing at all, exactly as an empty
one does. No harm is constructible from it here — such a record has no started instant to
un-fire, and `restore` drops the lease anyway — but the refusal covers what it covers, and the
rest is the operator's clock to get right.

A restored record whose observed identity names another provider or account is refused with
`foreign-resource`, as is a create answer naming one. The provider and account of a resource key
are not the provider's to report: the controller chose them when it configured the seam. An
answer it cannot account for is not adopted at all — the record keeps no observation and its
obligation stays open — rather than recorded as something this scope owns.

## Ceilings and cost policy

`HostingPolicy` carries a resource ceiling (`max_active`, 1..=4096) and a time ceiling
(`max_lifetime_ms`, never zero — there is no "unlimited" spelling). A provision beyond the
resource ceiling is refused from the controller's own records, before any mutation is submitted.
A requested lifetime equal to the policy ceiling is admitted and one millisecond past it is
refused; a resource is inside its lifetime at its deadline instant and past it one millisecond
later. `DeploymentSpec::effective_lifetime_ms` is the **minimum** of the requested lifetime and
the policy ceiling, and the controller uses that one expression. The two only differ after a
restart into a tightened policy, which is the one place the minimum is observable and the reason
it must not be a maximum.
A resource past its effective deadline becomes `StopRequired` with reason `lifetime-exceeded`:
the ceiling obliges a stop, it does not perform one, because performing one silently is how a
controller destroys a resource an operator was still using.

`Declare` requires a `ComputeAuthorization` naming the `llm-cost` ledger and the compute
reservation the resource is admitted against, and refuses an authorization from a different
ledger. Provisioning an unauthorized resource is unrepresentable rather than merely discouraged.
The controller does not re-derive admission and cannot see the ledger; the caller must `Reserve`
and `Begin` there first and must consume the obligations this contract reports.

## Listing and validation allocate nothing

`HostingProvider::inventory` and `HostingCommand::Validate` are the two operations that must
never allocate. Reconciliation is how a restarted controller finds what it already owns; a
listing that could create a resource would make reconciliation the cause of the double-billing it
exists to prevent. The fake provider counts `allocations`, `submits`, `stops` and `lists`
separately so that the scenarios can state this as a fact rather than an intention.

## Where the un-driftable inventories stop

Every closed vocabulary here — `HostingError`, `Phase`, `StopReason`, `Dispatch`,
`ProviderState`, `Idempotency`, `Completeness`, `AmbiguousCreate` — is generated by one macro in
`closed.rs` from a single variant list, so each type's `ALL` inventory and its `code` mapping
cannot fall behind its variants inside Rust. That guarantee stops at the crate boundary, and two
things carry it further rather than leaving the gap implicit.

Four of those vocabularies are re-declared by hand in `spec/domains/hosting.yaml`. The
conformance adapter's own tests read that file and compare its variant lists against the Rust
inventories, so a variant added, renamed or removed on either side without the other fails.
What is still **not** checked: the view's `DeploymentFact.phase` and `.stop_reason` are typed
`String` in the specification, as every other domain in this repository types its view facts, so
ESS does not validate an observed value against the declared variants. The comparison above is
what stands in for that.

Separately, a published value nothing can produce is a promise to a reader that nothing keeps.
`every_published_refusal_is_emitted_by_some_real_sequence_of_calls` and its counterpart for
`StopReason` produce every variant through real public calls under an exhaustive match, so a
refusal or reason added without a way to reach it does not compile.

## A guard nothing can trip is the same defect

The mirror of a refusal nothing can emit is a guard nothing can trip: it reads as protection and
is checked by nothing, so it is not written here. Four places in this crate were on the wrong
side of that rule and each is now resolved one way or the other.

| Guard | What was done |
| --- | --- |
| `Stop`'s scope check on `observed.key` | not written — an out-of-scope identity cannot reach a record, and the three legs holding that up are now each measured: `settle_create` refuses to adopt one, `adopt` refuses to take one from a listing, and `restore` refuses a snapshot carrying one |
| `elapse`'s terminal skip | deleted — `require_stop` refuses every terminal phase itself, so a skip in front of it decided nothing; the refusal is asserted where it lives |
| `discharge`'s `may_become(Stopped)` early return | kept, and made the one `ConfirmStopped` trips — the duplicate check in the command is gone, so the guard now decides the refusal instead of shadowing it |
| `fence`'s owner and epoch comparisons | deleted — at an equal epoch the registry's lease *is* ours, because an epoch is strictly monotonic and one epoch is granted to one owner. Deleting an untrippable guard only pays if what makes it untrippable is itself checked, so that invariant is now asserted on `LeaseRegistry` across acquisition, renewal, release, expiry and takeover. What `fence` still checks on its own is liveness: a claim that simply ran out with nobody taking over leaves the epoch untouched and is invisible to the comparison |

The three legs under the first row matter beyond their own coverage, because deleting `Stop`'s
scope check rests on them. Two of the three had no case aimed at them: `adopt`'s scope guard is
reached only through the request-id route, which searches the whole listing, and `restore`'s
check on the *requested* provider and account is a different fact from the `foreign-resource`
check on the observed identity. Both now have one.

## Verification and limits

[Hosting verification](verification/hosting.md) records the authored scenarios, the deliberate
defects and the counts. The ESS domain `llm-gateway.hosting` describes adapter observations of real
library calls; its notification and execution-record entities are verification plumbing, not a
claim that production publishes an event bus or uses an Entity Runtime store.

What this does **not** establish: that any real control plane behaves as `FakeProvider` does,
that a provider's listing is truthful, that a provider's termination report means billing
stopped, or that any cloud account was ever contacted. Runpod and Modal adapters, their
documented control-plane evidence, and live paid qualification are separate stories. The gateway
must still route hosting effects through the budget ledger and consume the stop obligations this
contract reports.

## Runpod

`b10x-llm-runpod` is the first adapter behind this seam. It ports llmgw's pod lifecycle
(`src/runpod.rs`, the Runpod part of `src/config.rs`, and the mock lifecycle tests in
`src/lib.rs`) onto the contract above. Every Runpod call goes through the `RunpodTransport` trait;
the only transport in the crate is `EmulatedRunpod`, an in-process control plane. Nothing in the
crate opens a connection or reads a credential. The crate links `b10x-llm-credentials` and `tokio`
through `b10x-llm-providers`, and uses neither: it calls only `descriptions::runpod` and
`inference_base_url`, which parse the shipped description and fill its URL template.

| Mechanism | llmgw | here |
| --- | --- | --- |
| identity | the pod name `llmgw-<alias>` | a `ResourceKey` whose incarnation is the Runpod pod id; owner, epoch and request id are written into the pod environment (`B10X_LLM_OWNER`, `B10X_LLM_EPOCH`, `B10X_LLM_REQUEST`) and read back from the listing |
| namespace | `llmgw-` | `b10x-llm-`; a name starting with `llmgw-` is never listed, adopted, swept or terminated |
| adoption after restart | any pod with the matching name | only the durable record's exact key, or its request id; a pod labelled for another owner is not adopted, and one taken over at a newer epoch is reported as transferred. After a takeover — the snapshot's previous lease holder gone and its claim expired — the new owner finds its inherited pods still labelled for that holder, because Runpod cannot relabel a pod; the contract parks such a record in `ownership-lost`, and the pool terminates the pod itself, but only when its label still names exactly the holder the snapshot records. A pod labelled for any other controller is never terminated |
| single-flight start | an atomic phase flip on a watch channel | one mutex across a whole pool step: the first caller declares and submits, every later caller sees the live deployment and is told `starting` |
| GPU choice | ordered; each refusal tries the next | the same, except that a **lost** create answer ends the attempt — Runpod takes no idempotency key, so trying the next GPU could pay twice, and `honours_idempotency_key` is `false` |
| crash recovery | two decreasing uptimes during startup terminate the pod | the same count, at any time, but only restarts inside the model's declared `crash_window_ms` count (window end inclusive), so sparse restarts over a long life are not a loop; a pod refusing its vLLM key is terminated too; the next request starts a new deployment |
| startup deadline | the caller's hold budget; the pod keeps starting | the pod's deadline is separate from a request's hold budget (`request_hold_seconds`, row K28): a pod that has not served within its declared deadline is terminated, because it is a billed resource serving nobody. A pod that served and then answers a definite "not ready" gets the same bound, counted from the last time it was seen ready (`stopped-serving`); unknown readiness neither starts nor ends that clock |
| idle reaper | idle past the timeout, never below the measured cold start, never with a request in flight | the same rules, on the explicit clock; a `StreamLease` holds the pod until the stream ends, and in-flight accounting is per deployment, so a lease still open on a retired pod does not hold its replacement |
| orphan sweep | `llmgw-*` pods whose alias left the registry | pods in `b10x-llm-` carrying **this controller's** owner tag that no live record holds, by exact key, or by request id for a record that holds no key yet; records whose model left the registry are stopped through the controller |
| request id | none | a readable prefix of at most 64 bytes (`request-<controller>-<alias>-<generation>-`, cut short when long) followed by a 128-bit digest of a length-prefixed encoding of controller, alias, generation, instant and a per-process nonce: at most 96 bytes, so it always fits the identifier limit. Two different tuples never share an encoding — `c-qwen`/`x` and `c`/`qwen-x` do not — so two request ids agree only if the digest collides. The digest is the standard library's hasher run twice, not a cryptographic hash; it has to avoid accidents, not an adversary |
| pod status | — | only `TERMINATED` ends a resource. `EXITED` is a stopped pod that still exists and is billed: it is reported present-but-not-running, terminated, and replaced (`pod-exited`). An endpoint is reported only for a `RUNNING` pod; otherwise it is `None` |
| endpoint | — | `https://<pod id>-8000.proxy.runpod.net/v1/`, `/v1/` included, built by llm's Runpod provider description (`b10x-llm-providers` 0.5.0, `descriptions::runpod().inference_base_url(<pod id>)`); this crate holds no URL format of its own. A running pod whose id the description refuses (anything but 1-48 bytes of `[a-z0-9]`) reports no endpoint, as a pod that is not running does, and is observed not ready whatever its probe answers: no ready lease is handed out without an endpoint, and the startup deadline terminates the pod (`startup-deadline`) like one that never becomes ready. `EmulatedRunpod` issues `pod1`, `pod2`, … |
| vLLM key | derived from the Runpod API key and passed as `--api-key` | the pod gets a Runpod secret reference in `VLLM_API_KEY` (`{{ RUNPOD_SECRET_<name> }}`), and no key value passes through `b10x-llm-runpod`. The binary does hold the value: it reads each model's `vllm_api_key_file` at startup (`crates/llm-gateway-cli/src/keys.rs`) and the relay sends it to the pod as `authorization: Bearer <key>` (row B8) |

Runpod settings — ordered GPU types, cloud type, disk, the mounted network volume and its
`HF_HOME`, data-center pinning, startup deadline, idle timeout, crash-restart limit and every
vLLM argument — are `RunpodModel`, not `DeploymentSpec`. A create whose image differs from the
declared one, or names an undeclared model, is not sent.

The pool must be stepped (`ensure` or `reap`) more often than the policy's `lease_ms`: an expired
lease is a stop obligation under this contract, and the pool carries it out.

### The pool the binary composes

`llm_gateway_cli::start_relaying` composes one `RunpodPool` from the deployment document and
relays each model to the pod it hands out, with the model's vLLM key as the bearer (row B8). The
shipped binary has no Runpod transport and never calls it; story:live-runpod-wiring supplies one.
Its tests run it over `EmulatedRunpod` and a loopback pod. Every input is fixed or read from the
document (`spec/domains/deployment.yaml`):

| Input | Source | Value |
| --- | --- | --- |
| `ComputeAuthorization` | fixed | ledger `b10x-llm-gateway`, reservation `unmetered`; no budget ledger is consulted yet |
| clock | fixed | `WallClock`: system time in Unix milliseconds, never moving backwards |
| `LeaseRegistry` | fixed | one empty in-process registry per run |
| `HostingPolicy` | fixed, and the document | controller `b10x-llm-gateway`, provider `runpod`, account `default`, ledger `b10x-llm-gateway`, `max_lifetime_ms` 86400000 (24 h), `lease_ms` 300000 (5 min); `max_active` is the number of declared models |
| `RunpodModel` | the document, and fixed | `hf_model`, `image`, `gpu_types`, `disk_gb`, the volume, `data_center_ids` and the vLLM settings from `[models.<alias>]`; `cloud_type` from its provider; `startup_deadline_ms` from `start_wait_seconds`; `idle_timeout_ms` from `idle_timeout_minutes`. Fixed: `crash_restart_limit` 2, `crash_window_ms` 600000, and `api_key_secret` `vllm_<alias>` with every `-` and `.` written `_` |

Every relayed model must name a `vllm_api_key_file` (`config:value` otherwise), because its pod
always expects the key. The pool runs one cleanup pass before the gateway is marked ready, which
sweeps what a previous run of this controller left (row L15), and then one every 60 seconds until
the stop (row L13), five passes to one `lease_ms`. `l15_*` and `l13_*` in
`crates/llm-gateway-cli/tests/relaying.rs` and `src/relaying.rs` prove both.

**A request is held while its pod starts** (row L6), and **its hold budget is its own setting**
(row K28). `request_hold_seconds` bounds how long one request waits; `start_wait_seconds` bounds
how long the pod may take to serve before it is terminated. llmgw uses one value for both; they
are separate here because a client with a short timeout of its own needs a short hold while the
pod keeps its long deadline (`docs/design/runpod-clients.md` recommends 240 s for Codex and Claude
Code, whose own timeouts are 300 s and 600 s). The hold defaults to `start_wait_seconds`, so a
document that does not name it behaves as llmgw, and is at most `start_wait_seconds`
(`config:value` past it), because a request still waiting past the deadline would wait for a
terminated pod. While the pool answers `starting` (or `stopping`, while the previous pod is being
stopped for a replacement) the relay asks it again every 500 ms through one `Hold`
(`RunpodPool::ensure_held`); each ask is one pool step under the pool's lock and the wait is
outside it. The hold binds to the pod its request first found starting, so requests held
together start one pod; a request that arrives during an unconfirmed stop waits unbound and binds
to the replacement it starts once the stop is confirmed. A request whose bound pod is still
starting when its hold passes is answered `model-cold-start` (503) with `retry-after: 30`
(row W6), and the pod keeps starting for the next request. A request whose hold passes while it
is still unbound had no pod starting for it, and is answered `target-unavailable` without
`retry-after`. If the bound pod fails during the hold (startup deadline, exit, crash loop,
refused key), is marked stop-required or leaves the slot, every request held on it is answered
`target-unavailable` at once, whichever request's or cleanup pass's step retired it, and none
starts a replacement; the next request does. A record whose create answer was lost
(`uncertain`) keeps its holds waiting, because its pod may still be adopted and serve. A hold of
0 asks once, starting the pod if none is live, and answers `model-cold-start` at once. `l6_*`, `w6_*` and `k28_*` in
`crates/llm-gateway-cli/tests/relaying.rs` prove it.

**`idle_timeout_minutes = 0` means no idle grace** (row K27). The pod is stopped by the first
cleanup pass that finds it with no request in flight, and never sooner than its measured cold
start, the floor every idle limit has. `RunpodModel` takes no zero window, so 0 is written as
1 ms. `k27_*` in `crates/llm-gateway-cli/tests/relaying.rs` prove both bounds.

A pod counts as in use while a request holds it and while one waits for it: its idle clock starts
no earlier than the step that first finds it ready, so the request that started it, still waiting
in `acquire`, finds it standing (`l10_a_pod_is_idle_only_from_the_step_that_first_finds_it_ready`
in `crates/llm-runpod/tests/runpod.rs`). A pod nobody comes back for is stopped once its idle limit
has passed after that step.

A request through the relay that fails drops its pod (row W7): the gateway reports the failed
authority to `invalidate`, and `RunpodPool::invalidate` stops the model's current pod if its
endpoint is that authority's, so the next request starts a replacement. A report about a pod
already replaced stops nothing.

During the stop a request may still use a pod that is ready now (`RunpodPool::ensure_running`),
but starts no pod and waits for none: with no ready pod for its model, or still waiting for one
when the stop begins, it is answered `target-unavailable` at once, so the graceful stop is not
held for `request_hold_seconds`.

**Pods outlive the process.** Stopping the gateway stops no pod: a pod created during the run keeps
running, and billing, after the process has exited. The next start's cleanup pass, run before the
gateway is marked ready, terminates every pod this controller created that no live record holds,
which after a restart is all of them.

**Open: orphan and inherited terminations bypass the controller.** An inherited pod whose record
is parked in `ownership-lost` is terminated through the provider too, because the controller
refuses to stop a resource labelled for another owner; its record is then discharged by the next
complete listing, with absence as the evidence. An orphan has no deployment record, so the
sweep calls `RunpodProvider::stop` directly, with the epoch read from the pod's own tag. No stop
obligation is opened or discharged for it and no receipt reaches the budget ledger: the ledger
never learns that the resource existed or that it was stopped. Closing that needs a change to
`llm-provision` — a way to record and discharge an obligation for a resource that has no
deployment — and is recorded as an open finding for the operator.

What this does **not** establish: that Runpod's REST listing returns the `env` a pod was created
with, that `{{ RUNPOD_SECRET_<name> }}` is substituted into `VLLM_API_KEY`, that a `DELETE` stops
billing, or that the proxy URL `https://<pod id>-8000.proxy.runpod.net/v1/` is the one to serve from.
Those are properties of the live control plane; no production transport exists yet and no paid
call has been made. [Runpod verification](verification/runpod.md) records the emulated evidence.
