# Gateway authentication verification — 2026-09-21

The [gateway contract](../gateway.md) defines the implemented authenticated single-owner surface:
authentication, liveness, readiness, graceful shutdown and read-only route inspection. This
checkpoint makes no model, provider or hosting call, resolves no secret and creates no resource.

## Counted coverage

`cargo test -p b10x-llm-gateway --locked` runs **44** cases across the six sources this lane
table covers, and 52 in the whole crate once `src/server.rs` and `tests/adversary_pass_2.rs` are
included. **All 52 pass**, and the whole-crate command exits 0. story:gateway-binary added two of
them to `tests/gateway.rs`: a request head is read, and a response written, under one deadline
each.

| Lane | Base | Now | Result |
| --- | --- | --- | --- |
| `unittests src/lib.rs` | 0 passed | 17 passed | ok |
| `tests/gateway.rs` | did not exist | 21 passed | ok |
| `tests/dependency_boundary.rs` | did not exist | 3 passed | ok |
| `tests/adversary.rs` | did not exist | 2 passed | ok |
| `tests/adversary_pass_2.rs` | did not exist | 7 passed | ok |
| doc-tests | 0 passed | 0 passed | ok |

The case that was red is described under *The one case that was red* below: it pinned three facts that
cannot all hold, so no behaviour satisfies it, and the documentation defect it reports is fixed.
Three consecutive runs printed identical counts. `cargo clippy -p b10x-llm-gateway --all-targets
--all-features --locked -- -D warnings` and `cargo fmt -p b10x-llm-gateway --check` both exit
zero.

An earlier version of this record described four cases in `tests/adversary.rs` as shipped and
red and gave the suite as 39 cases. The coordinator retired those four after verifying that each
asserted a property of a mechanism this unit's correction had replaced; the retirement note at
the end of `tests/adversary.rs` records which replacement took over from which retired case, and
the audit of the retirement blinded each replacement to confirm its control is genuine. This
record now describes the suite that exists.

Correction round 2 added these cases. Each turns a sentence the crate published into something
a runner checks, and each is in `tests/gateway.rs`:

| Case | What it now checks rather than asserts |
| --- | --- |
| `every_numeric_bound_in_the_source_is_published` | the bounds table against every numeric constant the source declares, not against a second hand-written list |
| `a_stalled_head_is_refused_when_the_read_timeout_expires` | that the read timeout is enforced, with the default value asserted separately and not published as a bound |
| `the_crate_reaches_no_io_beyond_the_listener_it_was_given` | that no source file names a filesystem, subprocess, environment, outbound-socket or console API |
| `proof_of_authentication_is_a_marker_on_the_request_path_not_a_capability` | that the proof type is mintable and does not gate the data, which is what is true |
| `an_inspection_response_carries_exactly_these_keys` | the exact key set that reaches a client |

`src/server.rs` also gained `every_status_the_crate_can_answer_carries_a_reason_phrase`, which
covers the one property of a refusal the `refusal_codes!` macro does not generate.

There is **no ESS domain and no conformance adapter for this story**. The behaviour is proved by
ordinary Rust tests against a real loopback socket: every acceptance case opens a `TcpStream` to
a bound `127.0.0.1:0` port, writes raw HTTP/1.1 bytes and reads the response to EOF.

## What the acceptance is proved by

| Acceptance clause | Case |
| --- | --- |
| An unauthenticated request is refused | `an_unauthenticated_inspection_is_refused_and_discloses_no_route` |
| …and discloses nothing about the routes | the same case, over every identifier and limit in the fixture |
| A wrong or malformed credential is refused and not echoed | `a_rejected_or_malformed_credential_is_never_echoed_back` |
| An authenticated owner can inspect the routes | `the_owner_inspects_routes_without_a_credential_or_a_secret_read`, `a_single_route_is_inspectable_by_alias_and_an_unknown_alias_is_refused` |
| …revealing no upstream credential | the same case, over eight consecutive requests, plus the secret-read counter |
| …and triggering no provisioning | `the_transitive_closure_contains_nothing_that_can_resolve_a_secret_or_provision`, `the_declared_dependency_scan_reads_the_whole_array`, `the_manifest_names_nothing_forbidden` |
| Health, readiness and drain are distinct | `health_and_readiness_are_distinct_and_need_no_credential`, `liveness_is_answered_before_load_is_shed`, `a_drain_cannot_be_undone_by_marking_ready_again`, `graceful_shutdown_drains_readiness_then_stops_accepting`, `a_connection_accepted_before_shutdown_is_finished_rather_than_refused` |
| Unknown stays unknown | `an_unreported_limit_stays_absent_and_never_becomes_zero` |
| Every refusal is named, documented and reachable | `every_refusal_code_has_a_request_that_provokes_it`, `the_published_refusal_table_matches_the_codes_the_crate_can_emit` |
| Every bound is enforced at the number published for it | `every_published_bound_flips_at_exactly_the_published_number` |
| Every error variant is reachable | `every_token_error_has_material_that_provokes_it`, `every_verifier_error_has_a_composition_that_provokes_it`, `every_label_error_has_a_value_that_provokes_it`, `every_inventory_error_has_a_construction_that_provokes_it` |
| Every bound in the source is published and enforced | `every_numeric_bound_in_the_source_is_published`, `every_published_bound_flips_at_exactly_the_published_number`, `a_stalled_head_is_refused_when_the_read_timeout_expires` |
| Nothing is written anywhere but the socket | `the_crate_reaches_no_io_beyond_the_listener_it_was_given`, `an_inspection_response_carries_exactly_these_keys` |
| Every status reaching a client has a reason phrase | `every_status_the_crate_can_answer_carries_a_reason_phrase` |

**The dependency boundary.** Two independent mechanisms, each with a positive control that fails
if the mechanism stops seeing what it is supposed to see:

1. The declared list from `cargo metadata --no-deps`, read to the dependency array's **matching
   bracket**. The control reads `b10x-llm-conformance`, which really declares eighteen
   dependencies including `keyring-core` and `tokio`, and asserts both are found. The earlier
   version of this file split at the first `]` — which closes the first dependency's own
   `features` array — and so examined **one dependency of eighteen**.
2. The **transitive closure**, from `cargo tree --edges normal,build --all-features`, which cargo
   computes rather than this repository. The control reads `llm-routing` and asserts its closure
   really does reach `b10x-llm-credentials` and `tokio`. The gateway's own closure is asserted to
   be exactly one package: itself.

**The published contract is checked, not maintained.** `RefusalCode`, `ALL`, `wire`, `status` and
`reason` are generated from one list by the `refusal_codes!` macro, so a variant missing from any
of them is a compile error rather than a test. The remaining links — the document's code, status
**and** message columns, and a request that provokes each code — are checked at runtime, in both
directions, so a row naming a code the crate cannot emit fails too. Before this round the status
column was compared to nothing and six of ten statuses were asserted only against themselves.

**Bounds are measured at literal numbers.** `docs/gateway.md` publishes a bounds table, and every
entry is exercised at the published number and one past it. Before this round the shared-secret
minimum could be lowered from 32 to 8 with the suite green, because the case wrote
`MIN_SHARED_SECRET_BYTES - 1`; a bound asserted against its own constant asserts nothing.

## Claims of the form "this cannot be built, therefore this cannot happen"

Three separate reviews found a true premise carrying a false conclusion in this crate, so the
crate's own documentation was swept for the shape rather than the instance. Four were found, and
each is now either true or restated as what holds, with a case pinning it:

| Claim | Verdict | What replaced it |
| --- | --- | --- |
| `src/inventory.rs`: no field for a URL or a secret reference, so no response can carry one | **false** — `Label` is any printable ASCII | the guarantee is that a response carries exactly the identifier bytes the composer supplied; `provenance_renders_exactly_the_bytes_the_composer_supplied` pins it, and the obligation is written as the adapter's |
| `src/error.rs`: every variant is provoked by a case, because the tests match exhaustively over `ALL` | **false** — `ALL` was hand-written for four error types, and an exhaustive match forces an *arm* for a new variant without `ALL` ever reaching it | `closed_enum!` generates each enum with its `ALL`, so the two cannot diverge; `error-variant-removed-from-the-generating-list` is a compile error |
| `src/auth.rs`: the proof cannot be constructed except from the verdict, so no inspection is reachable before authentication | **false** — `Verdict::Owner` is public, and `RouteInventory::route` reaches the same bytes with no proof | the proof is a marker on the gateway's own request path; the statement that holds is about the HTTP surface, and `proof_of_authentication_is_a_marker_on_the_request_path_not_a_capability` pins both halves |
| `src/auth.rs`: the crate cannot read a keychain, open a file or reach a network | **imprecise** — it binds and accepts a socket by design | the crate's source names no filesystem, subprocess, environment, outbound-socket or console API, which `the_crate_reaches_no_io_beyond_the_listener_it_was_given` now checks against the source |

The remaining claims of this shape were checked and hold: the JSON writer has no derive, so a
struct field reaches no client unless an explicit call writes it (`an_inspection_response_carries_exactly_these_keys`
pins the key set); the snapshot holds no callback, so rendering it provisions nothing (the
dependency boundary carries that); and a drain cannot be undone
(`a_drain_cannot_be_undone_by_marking_ready_again`).

## Falsification

Thirty-two mutations were applied one at a time to the real source, the suite was run, and each
file was restored and its SHA-256 compared with the value recorded before the mutation. Kills are
counted against the single case that is red on the unmutated tree. The full record, with exact
before/after text and per-mutation counts, is in
[gateway-falsification.json](gateway-falsification.json).

**Thirty-two of thirty-two killed a named case. Nothing survived.** The one mutation that
survived the previous round — writing the presented credential to process stderr — is now killed
by `the_crate_reaches_no_io_beyond_the_listener_it_was_given`, which was the recorded coverage
boundary and is a check rather than a boundary now.

Two mutations kill by **compile error**, which is the strongest form: removing a refusal code or
an error variant from its generating macro removes the variant itself.

The mutations added this round, and what each proves:

| Mutation | Killed by | What it establishes |
| --- | --- | --- |
| `request-head-arithmetic-off-by-one` | `every_published_bound_flips_at_exactly_the_published_number` | the head bound is measured by enforcement over a socket; the previous probe read `GatewayConfig::new(..).max_head_bytes` and this mutation would have stayed green |
| `concurrency-comparison-off-by-one` | same | the concurrency bound is measured by occupying 63 and 64 slots, not by reading the default |
| `digest-bound-unpublished` | `every_numeric_bound_in_the_source_is_published` | the bounds table is compared against the source, so a bound in neither the table nor the measurement is no longer invisible |
| `reason-phrase-map-loses-a-status` | `server::tests::every_status_the_crate_can_answer_carries_a_reason_phrase` | the one property of a refusal the macro does not generate is now compared to the codes' statuses |
| `read-timeout-not-applied` | `a_stalled_head_is_refused_when_the_read_timeout_expires` | the timeout is enforced, not merely defaulted |
| `error-variant-removed-from-the-generating-list` | compile error | `closed_enum!` generates each error enum with its own `ALL` |
| `rejected-credential-echoed-in-the-diagnostic` | `the_crate_reaches_no_io_beyond_the_listener_it_was_given` | a credential has nowhere in this crate to be written except the socket |

Everything falsified in earlier rounds still is: authentication, the constant-time comparison,
every structural bound, the unknown-stays-unknown rendering, the drain and shutdown ordering,
the liveness ordering, the published refusal table in all three columns and both directions, the
declared-dependency scan and the transitive closure.

## The one case that was red

`a_connection_the_report_counts_as_completed_was_not_answered_with_a_refusal`, in
`tests/adversary_pass_2.rs`, is red and cannot be made green. It asserts
`report.completed == 1`, observes a `401 credential-absent` on the only connection, and then
asserts that response was not a refusal. No behaviour satisfies all three: a report that omitted
the refused connection would fail its second assertion instead.

Its finding is correct and is fixed. `docs/gateway.md` said, without qualification, that a
connection the report counts as completed was not answered with a refusal. `completed` counts
every accepted connection, including ones refused on their own merits. The sentence now says
that no connection the report counts as completed was refused **because of the stop**, which is
the invariant the shutdown ordering actually provides and the one the pass-1 blocker asked for.
The ordering itself is correct and is falsified by
`shutdown-clears-readiness-before-draining-in-flight`.

## Qualification boundaries

- **The forbidden-dependency list is a list.** The transitive check is sound, but its ten names
  are chosen by hand. A crate that can resolve a secret and is not on that list passes.
- **The forbidden-API list is a list too.** `the_crate_reaches_no_io_beyond_the_listener_it_was_given`
  names the ways a zero-dependency crate can write a byte elsewhere; it is complete only while
  the crate keeps declaring no dependencies, which the boundary case enforces.
- **`transport-crate-named-in-the-manifest` mutates the manifest's description, not its
  dependency table.** `Cargo.lock` is frozen in this working tree and `cargo metadata --locked`
  exits 101 before any test runs. The transitive mechanism is falsified by
  `transitive-closure-walk-stops-at-the-root` instead.
- **Taking the `llm-routing` dependency costs the structural guarantee.** Its closure reaches
  `b10x-llm-credentials` and `tokio` two edges away, and the boundary case fails if that is ever
  done silently.
- **Zeroisation of `OwnerToken` is best effort.** No `zeroize` dependency; the drop overwrites
  the buffer and passes it through `std::hint::black_box`. Not a guarantee against a
  reallocation that already copied the bytes.
- **Constant-time comparison leaks length.** It folds the length difference and every
  overlapping byte, so it does not leak *where* a credential differs, but the time depends on
  the presented length.
- **Overload is shed after the request head is read**, because the probe paths must be matched
  first. A flood still costs a bounded head read per connection.
- **The read timeout's default value is asserted, not exercised.** Ten seconds is too slow for
  the suite, so enforcement is measured with a configured 150 ms and the default is compared as
  a value. It is deliberately absent from the bounds table for that reason.
- **No TLS and no keep-alive.** Plaintext HTTP/1.1, one request per connection. Not qualified
  for exposure to an untrusted network.
- **`llm-routing` is not wired in.** The `Catalog` to `RouteInventory` adapter belongs to the
  composing binary and does not exist yet, so no case exercises a real parsed catalog.
- **No spec domain, scenario suite or conformance adapter** exists for `llm.gateway`.

### How that case was closed

It was not closed by this unit. The case asserted that a refusal's status was 401 and then that
the same status was below 400, so no behaviour could satisfy it. It was a demonstration that the
documented invariant was false as written, and it worked: the sentence is now narrowed to say that
no connection the report counts as completed was refused *because of the stop*.

The coordinator re-pointed the assertion at that narrowed invariant, after the unit left the case
standing and proposed the change rather than editing an assertion itself. The re-pointed case
asserts that no completed connection carries the one refusal code a stop can produce. The unit's
own account of why the case could not be satisfied is left above, unedited, because it is the
reason the change was made.
