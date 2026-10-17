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

| Route | Evidence (`EvidenceKind`) |
| --- | --- |
| absent from a listing that can answer for this request | `inventory-complete-absent` |
| the provider reported the resource terminated | `provider-terminated` |
| an accepted stop mutation | the provider's own reference (`provider-reference`) |
| `ConfirmStopped` with an external observation | the caller's reference (`caller-reference`) |

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
The totals carry the ceiling's own count beside `stop_required` and `transferred`: `active_count`
is the records occupying a slot, an uncertain one included because it may be billed,
`unknown_count` the uncertain ones, and `at_capacity` whether `active_count` has reached
`max_active`.
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
`ProviderState`, `Idempotency`, `Completeness`, `EvidenceKind`, `AmbiguousCreate` — is generated
by one macro in
`closed.rs` from a single variant list, so each type's `ALL` inventory and its `code` mapping
cannot fall behind its variants inside Rust. That guarantee stops at the crate boundary, and two
things carry it further rather than leaving the gap implicit.

Seven of those vocabularies are re-declared by hand in `spec/domains/hosting.yaml`, with the
transition table as `Transition`, and `HostingError` with each code's message. The conformance
adapter's own tests read that file and compare its variants and their attributes against the
Rust inventories, so a variant added, renamed or removed on either side without the other fails.
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
`src/lib.rs`) onto the contract above. Every Runpod call goes through the `RunpodTransport` trait.
The crate has two transports: `EmulatedRunpod`, an in-process control plane the tests drive, and
`ConnectorsRunpod`, the production transport described below. Neither holds a control-plane
credential, and no test reaches a live API. The crate links `b10x-llm-credentials` and `tokio`
through `b10x-llm-providers`, and uses neither: it calls only `descriptions::runpod` and
`inference_base_url`, which parse the shipped description and fill its URL template.

| Mechanism | llmgw | here |
| --- | --- | --- |
| identity | the pod name `llmgw-<alias>` | a `ResourceKey` whose incarnation is the Runpod pod id; owner, epoch and request id are written into the pod environment (`B10X_LLM_OWNER`, `B10X_LLM_EPOCH`, `B10X_LLM_REQUEST`) and read back from the listing |
| namespace | `llmgw-` | `b10x-llm-`; a name starting with `llmgw-` is never listed, adopted, swept or terminated |
| adoption after restart | any pod with the matching name | only the durable record's exact key, or its request id; a pod labelled for another owner is not adopted, and one taken over at a newer epoch is reported as transferred. After a takeover — the snapshot's previous lease holder gone and its claim expired — the new owner finds its inherited pods still labelled for that holder, because Runpod cannot relabel a pod; the contract parks such a record in `ownership-lost`, and the pool terminates the pod itself, but only when its label still names exactly the holder the snapshot records. A pod labelled for any other controller is never terminated |
| single-flight start | an atomic phase flip on a watch channel | one mutex across a whole pool step: the first caller declares and submits, every later caller sees the live deployment and is told `starting` |
| GPU choice | ordered; each refusal tries the next | the same within a profile, except that a **lost** create answer ends the attempt — Runpod takes no idempotency key, so trying the next GPU could pay twice, and `honours_idempotency_key` is `false`. A model may declare ordered profiles (below); the next profile is tried only when every create of the one before was refused for capacity |
| crash recovery | two decreasing uptimes during startup terminate the pod | Runpod reports no uptime, so a restart is the pod's `lastStartedAt` (read by `pods.list` with `id`) moving forward: a value later than the last one seen is one restart, and the first value, an equal or an earlier one, or none, is not. The same count, at any time, but only restarts inside the model's declared `crash_window_ms` count (window end inclusive), so sparse restarts over a long life are not a loop; a pod refusing its vLLM key is terminated too; the next request starts a new deployment |
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

The crash-restart limit is a per-model setting, `crash_restart_limit` (at least 1), not a fixed
two: a pod is a crash loop once the restarts seen inside its `crash_window_ms` reach it, and
llmgw's two decreasing uptimes are the limit 2, which the shipped binary fixes. A pod Runpod
reports `EXITED` is observed in `ProviderState::Pending`, as a pod still being created is:
present and not running. `pod-exited` and `stopped-serving`, like `crash-loop`,
`credential-refused` and `startup-deadline`, are refusals a caller receives: the one call of
`ensure`, `ensure_running` or `ensure_held` whose step retired the pod is told why, a caller
held on that pod is answered `stopping`, and a cleanup pass that retires a pod refuses nobody
and reports it as `replaced`; the gateway answers each of them `target-unavailable`.

A model declares one or more profiles: its own `gpu_types` and `vllm` are the first, named
`profile` (`default` for a deployment document without `profiles`), and `fallbacks` the rest, in
order (`RunpodProfile`: name, GPU types, vLLM settings). A start tries the GPU types of the first
profile in order, the spend check immediately before each create. A create refused for capacity
(`CreateAnswer::NoCapacity`, the connectors failure code `capacity`) or for any other reason
(`Refused`) moves on to the profile's next GPU type. When every create of a profile was refused
for capacity, the next profile is tried; a profile with any other refusal ends the start, as do a
lost create answer and a spend refusal. Every profile is tried inside the one create of the one
start, so one startup deadline, counted from that submission, covers them all, and binds the
creates: before each create, of the profile's next GPU type and of the next profile, the provider
reads the clock, and once more than `startup_deadline_ms` has passed it sends none and the start
ends as one whose every create was refused (`no-capacity`, the deployment withdrawn). A GPU type belongs
to at most one profile of a model, so a pod's GPU type names its profile, after an adoption too:
`StreamLease::profile` reports it, and the gateway's usage record carries it as `profile`.

The pool also answers `stopping` while a model's deployment owes a stop that is not confirmed,
and `no-capacity` when every declared GPU type refused its create, or every one tried before the
start's startup deadline passed; neither creates anything. It
answers `unknown-model` for an alias nobody declared, without contacting Runpod,
`invalid-model` when a declaration is refused, and `hosting` when the controller refuses. A
model declaration is refused with `no-gpu` when it names no GPU type, `gpu-utilization` for a
`gpu_memory_utilization` outside (0, 1], `zero-setting` for a zero count, limit, deadline or
window, `secret-reference` for a secret name that is not `[A-Za-z0-9_]+`, `unsafe-text` for
an empty or unprintable value bound for the pod's command line or environment, or a profile name
that is not 1 to 32 of `A-Za-z0-9-_`, and `profile-conflict` when two profiles share a name or a
GPU type. Every fallback profile is checked as the model is.

The pool must be stepped (`ensure` or `reap`) more often than the policy's `lease_ms`: an expired
lease is a stop obligation under this contract, and the pool carries it out.

### The production transport

`ConnectorsRunpod` reaches Runpod's control plane only through the `connectors` CLI of
beyond10x/connectors v0.37.0 and its Runpod catalog bundle (connectors `docs/catalog-runpod.md`).
The Runpod API key stays in the connectors keyring; this crate holds no HTTP client for the
control plane and no credential for it. `spec/domains/runpod.yaml` declares every invocation.

| Call | connectors operation | Answer |
| --- | --- | --- |
| list | `pods.list` with no filter | `complete` only when the whole answer is one array of pods and every pod parses; a truncated, throttled (`rate_limited`), interrupted or partly unreadable answer is not complete |
| get | `pods.list` with `id` (v0.37.0 selects no `GetPod`) | the pod's `lastStartedAt`, for crash detection |
| create | `pod.create`, one per GPU choice | `Created` for `applied` with a readable pod; `NoCapacity` for `refused` with the failure code `capacity`, `Refused` for any other `refused`; `unknown` is resolved by one `pods.list` on the pod's `name`, which is per model alias (`Created` only if exactly one pod of that name carries this create's `B10X_LLM_REQUEST` tag, `Lost` otherwise); `not_attempted`, a timeout, a non-zero exit with no classification and unparseable output are `Lost` |
| terminate | `pod.terminate` with `podId` | `Terminated` for `applied`; `Refused` for `refused`, including `404`/`not_found`, which does not prove the pod is gone; anything else `Lost` |

Each write is prepared and issued for its exact input (`approvals prepare`, `approvals issue`
into a new file in the transport's private work directory, made absolute when the transport is
built) and invoked once with that proof and an idempotency key; the transport never sends it
again. `schema` and `revision` come from `operations describe` and are kept per operation until
one of the refusals below drops them. The CLI runs in its own process group. When the binding's
timeout passes before the CLI has exited and both its output streams have closed, which a process
it started can hold open, the group is killed and the answer is lost.

Answers are read only in connectors' published framing (connectors v0.37.0
`contracts/cli/v1alpha1/semantics.md:178-181`). A success is exit 0 with one
`{"ok":true,"result":…}` envelope on stdout, and everything the transport reads (`schema`,
`revision`, the approval's `subject_sha256`, the connection's revision, the invoke's `status`,
`body` and `mutation.classification`) is inside its `result`. A failure is a non-zero exit with
stdout empty and one `{"ok":false,"error":{"code":"failure","data":…}}` envelope on stderr, whose
`data` is connectors' `Failure` (`code`, `stage`, `next_action`, and `mutation` when connectors
observed one). Both streams are read under the call's deadline and an 8 MiB bound each. Anything
else is unreadable: `Lost` for a write, an incomplete listing for a read. No part of either stream
reaches the transport's `Debug`.

An invoke refused with nothing dispatched (no classification, or `not_attempted`) is repeated
once, after one recovery step, for three refusals:

| `error.data` | Why (connectors v0.37.0) | Recovery |
| --- | --- | --- |
| `code: not_granted`, `stage: admission` | the connection's evidence expired: it lasts `evidence_lifetime_ms`, 60 s when omitted and at most 300 000 ms, and "The invoke does not revalidate on its own" (`docs/local-catalog-provider.md:479-486`, `semantics.md:748-753`) | `connections describe` for the connection's revision, then `connections revalidate` once |
| `code: stale_description` | "schema/descriptor changed", answered "before provider dispatch" (`semantics.md:579-581`, `:606`) | drop the operation's cached `schema` and `revision`, run `operations describe` once |
| `code: lifecycle_conflict`, `stage: admission` | "stale revision" (`semantics.md:601`), as after a configuration upgrade moves the descriptor revision (`docs/local-catalog-provider.md:396-399`, `semantics.md:759-764`) | the same refresh, and a revalidation too when `next_action` is `revalidate_connection` |

A recovery step that fails ends there. A repeated write is a new attempt, with a fresh proof and
its own idempotency key. A second refusal is mapped as usual (`Lost` for a write, an incomplete
listing for a read), and a write refused any other way, or classified otherwise, is never
repeated.

The `pod.create` body carries the keys `POD_CREATE_BODY_KEYS` names, which are connectors
v0.37.0's fifteen `body_keys` (`adapters/catalog/providers/runpod/operations.json`); connectors
refuses any other key as `invalid_input` before any request (`docs/catalog-runpod.md:88-89`). The
body follows llmgw's (`src/runpod.rs:781-799` at `048ebd8`). The vLLM argv (`vllm serve …`)
travels as `dockerEntrypoint`, `dockerStartCmd` is always `[]`, `volumeInGb` is `0` when there is
no network volume (Runpod's default is a 20 GB pod volume, `docs/catalog-runpod.md:77`), and the
model cache volume travels as `networkVolumeId`, which "replaces the pod volume" (:86).
`dockerStartCmd: []` is llmgw parity, not a way to clear the image's CMD: Runpod's pinned
`PodCreateInput` says "If [], uses the start CMD defined in the image", and connectors v0.37.0
says `[]` "keeps the image's" (`docs/catalog-runpod.md:84`).

The readiness probe goes to the pod, not to connectors: `GET <endpoint>models` with the model's
vLLM key, handed to the transport by alias, as a bearer. `200` is ready with the served model
names, `401`/`403` refused, any other answer not ready, no answer unreachable. An `http://`
endpoint is probed over a plain socket. An `https://` endpoint, which is what a live pod has, is
probed over the TLS connection the embedding hands the transport (`ConnectorsRunpod::set_tls`):
this crate links no TLS client, and the binary's verifies the certificate before the probe writes
its request or the key. Without one, or when the handshake fails, the endpoint is unreachable. The
transport reads no file, and its `Debug` prints no key.

### The pool the binary composes

`llm_gateway_cli::start_relaying` composes one `RunpodPool` from the deployment document and
relays each model to the pod it hands out, with the model's vLLM key as the bearer (row B8). Its
tests run it over `EmulatedRunpod` and a loopback pod. The composition the shipped binary is
wired for, `llm_gateway_cli::start_connected`, builds a `ConnectorsRunpod` from the document and
runs the same pool over what its caller's `wrap` returns: `llm_gateway_cli::start`, the shipped
path, passes the identity, and only tests pass anything else. It serves the one provider whose
`[providers.<name>.connectors]` table names the connection, and makes every document check
before its first `connectors` call:

```toml
[providers.runpod.connectors]
executable = "/usr/local/bin/connectors"   # absolute; not looked up on PATH
adapter = "gpu"                            # the alias bound to the Runpod catalog provider
connection = "conn-1"                      # the saved connection id
work_directory = "/var/lib/gateway/connectors"  # existing, owned by the gateway's user, mode 0700
timeout_seconds = 60                       # 1..=600, default 60: one CLI call
```

**A pod is reached over verified TLS.** The shipped binary reaches a pod only at the Runpod
proxy's `https://<pod id>-8000.proxy.runpod.net/v1/` (port 443), for the readiness probe and for
every relayed request. Its TLS client is `rustls` on the `ring` provider, in
`b10x-llm-gateway-cli` only, and it trusts exactly the Mozilla root program as the `webpki-roots`
crate compiles it in: no system certificate store, file or environment variable. The handshake,
chain and host name verification included, completes before any request byte or key is written,
so a pod whose certificate does not verify reads nothing: the relayed request is answered
`upstream-failed`, and the probe reports the pod unreachable. A pod connection gives up connecting
after 10 s, one deadline over the TCP connection and the handshake together, so a pod that
answers slowly or not at all cannot stretch it; once open, a relayed connection's reads and writes
each time out after 120 s. No document key and no option adds a root or disables verification;
the closed document refuses any such key as `config:schema`. The `gateway-tls-seam` test program,
which trusts a root its environment names, is built only with the test-only `test-seam` feature
that the package's tests enable: `cargo build --release -p b10x-llm-gateway-cli` does not build it.

What `start_connected` does, proved at the library level: at start it checks the connection once
(the work directory is private and one `pods.list` answers a complete listing). If not, it still
starts, writes the `warn` event `connectors connection unreachable`, runs no further `connectors`
call, and answers every model of that provider `target-unavailable`; a repaired connection takes a
restart. Each connected model's vLLM key is handed to the transport by alias for the readiness
probe. A model whose provider declares no `connectors` is `target-unavailable`, in the shipped
binary too. No option or document key selects `EmulatedRunpod`.
`crates/llm-gateway-cli/tests/pod_tls.rs` proves the binary end to end against the connectors CLI
fixture and a loopback `https` pod, through a test program that runs the shipped `main` with a
test root and the pod's proxy host mapped to loopback; `tests/live_runpod.rs` proves the
composition against the fixture and a plain loopback pod.

Every input is fixed or read from the document (`spec/domains/deployment.yaml`):

| Input | Source | Value |
| --- | --- | --- |
| `ComputeAuthorization` | fixed, and the document | ledger `b10x-llm-gateway`; reservation `spend-ceiling` when a provider declares a spend ceiling, `unmetered` otherwise |
| `SpendPolicy` | the document | `daily_cap_usd` and `total_cap_usd` of the one provider that declares any, and every model's `price_per_hour_usd`, in micro-USD ([spend ceilings](#spend-ceilings)) |
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

### Spend ceilings

A `[providers.<name>]` table may declare `daily_cap_usd` and `total_cap_usd`, the account's spend
ceilings in USD (each finite, above 0, at most 1000000 and at least 0.000001; `config:value`
otherwise). At most one provider declares one, because the pool is one account, and once one
does every `[models.<alias>]` declares `price_per_hour_usd` (above 0, at most 1000): the most its
pod costs per hour on any of its GPU types. Amounts are held as integer micro-USD, so a ceiling
compares exactly.

The spend check, `RunpodProvider::admit_spend`, runs immediately before every create the
provider sends, once per declared GPU type it tries, and needs no relayed request. Without a
ceiling it reads nothing. With one it adds, at the pool clock's instant:

- the account's reported spend, read afresh for each check: today's since the UTC day's start,
  and the total;
- the running estimate of the pool's live pods: for every record of this controller that
  occupies a slot, its model's price times the time since its create was submitted, and never
  less than its price times its startup deadline, so a pod submitted moments ago counts what
  its own start was admitted at and two starts in a row never share one headroom;
- the estimate of the pod about to start: its model's price times its startup deadline.

Each estimate is rounded up to the next micro-USD; a reported amount and a declared ceiling or
price are rounded to the nearest, so an amount billed in whole cents is read exactly. The price
is always the declared one, never the pod listing's. The daily projection is compared with the daily ceiling first, then
the total one with the total ceiling. A projection equal to its ceiling is admitted; one
micro-USD past it is refused. A spend read that fails or answers nothing, or a model with no
price, is refused too: no ceiling is ever assumed met. A refused check sends no create and tries
no further GPU type; the deployment is withdrawn (`cancelled`), the pool answers
`spend-ceiling`, the request is answered `spend-ceiling` (503) at once without `retry-after`,
`llmgw_pod_start_spend_refusals_total` counts it, and a `warn` event, `pod start refused by a
spend ceiling`, names the model, the ceiling (`daily`, `total` or `unknown`) and the excess in
USD. The client's message names neither.

The check errs toward refusing, and its billing assumptions are unchecked against a live
account:

- The running estimate counts a live pod's whole time since submission, so it may count again
  hours the reported spend already holds.
- A pod this pool holds no record for (another controller's, or a previous run's before its
  orphan sweep) is counted only once the reported spend holds it.
- The reported spend is Runpod's `GET /billing/pods` (`PodBilling`) with `bucketSize=day`: each
  entry's `amount` is taken as USD and its `time` as the bucket's start. Neither reading has been
  checked against a live account.
- The total is the sum of `billing.pods` over the provider's default time window, since the
  read names none. If that window is bounded, spend billed before it is not counted and the
  total ceiling is understated.
- A `billing.pods` result is read only when it carries a 2xx `status`; one with no status is
  unanswered, never zero spend, and refuses the start.

**The live spend read needs a connectors release.** `ConnectorsRunpod` reads the account's spend
through the connectors operation `billing.pods`, which the connectors v0.37.0 Runpod catalog does
not select. Until a connectors release offers it, `operations describe` refuses it, every spend
read is unanswered, and a deployment that declares a ceiling starts no pod: every start is
answered `spend-ceiling`. A deployment that declares none is unaffected. `spend.rs` and the
`billing_*` cases of `transport.rs` in `crates/llm-runpod/tests`, the ceiling scenarios under
`contracts/runpod/scenarios`, and `spend_*` in `crates/llm-gateway-cli/tests/config.rs` and
`tests/relaying.rs` prove it.

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
billing, or that the proxy URL `https://<pod id>-8000.proxy.runpod.net/v1/` is the one to serve from
and presents a certificate that chains to a root the binary ships.
Nor does it establish that `lastStartedAt` moves when a container restarts. Those are properties
of the live control plane; the production transport has run only against a fixture of the
`connectors` CLI and no paid call has been made. [Runpod verification](verification/runpod.md)
records the emulated evidence, [the transport record](verification/runpod-transport.md) the
fixture evidence.
