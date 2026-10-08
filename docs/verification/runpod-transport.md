# Runpod production transport verification — 2026-10-08

The [production transport](../hosting.md#the-production-transport) section describes
`ConnectorsRunpod`, and `spec/domains/runpod.yaml` declares its invocations. Every case here
drives it against fixtures: a fixture of the `connectors` CLI and, for the readiness probe, a
loopback HTTP server. No case runs the real `connectors`, opens a socket beyond loopback, reads a
key, calls Runpod or starts a pod.

## The interface it was built against

beyond10x/connectors at tag `v0.36.0`, read from the release:

| Document | What it fixed |
| --- | --- |
| `docs/catalog-runpod.md` | the operations `pod.create`, `pods.list`, `pod.terminate`; no `GetPod`; the `pods.list` filters; `mutation.classification` per answer; the `pod.create` body keys; the `invoke` command lines |
| `docs/local-approvals.md` | `approvals prepare` → `approvals issue --approve-subject <sha256> --proof-output <new file in a private dir>` → `operations invoke --approval-file --idempotency-key`; a proof binds the exact input |
| `docs/local-catalog-provider.md`, `docs/local-gitlab-merge.md` | `schema` and `revision` from `operations describe`; `result` and `mutation` on success, `error.data` on failure |
| `ess/domains/cli.yaml`, `ess/domains/delegation.yaml`, `ess/domains/mutations.yaml` | the printed JSON: `preparation.subject_sha256`, `OperationInvokeResult`, `EffectKnowledge` |
| `adapters/catalog/providers/runpod/operations.json` | the twelve `body_keys` |
| `adapters/runpod/upstream/runpod-rest-v1.json` (SHA-256 `9500a898…b580db`) | `PodCreateInput` (`dockerEntrypoint`, `dockerStartCmd`, `networkVolumeId`), `Pod.lastStartedAt` (`2024-07-12T19:14:40.144Z`), `desiredStatus` |

## Commands

```sh
cargo test -p b10x-llm-runpod --locked --test transport
cargo test -p b10x-llm-runpod --locked
cargo clippy -p b10x-llm-runpod --all-targets --locked -- -D warnings
cargo fmt -p b10x-llm-runpod --check
```

## Fixtures

- `crates/llm-runpod/tests/fixture/connectors.rs`, built by cargo as the `connectors-fixture`
  binary. Each test links it as `connectors` into its own directory under cargo's
  `CARGO_TARGET_TMPDIR`; it records every invocation's argv and input document in `calls.jsonl`
  and answers from `script.json`. It answers `operations describe`, `approvals prepare` and
  `approvals issue` itself; an issued proof records the input it was issued for, and an invoke of
  a write with no proof, no idempotency key, or a proof for another input is refused and recorded
  as `proof_mismatch`. It answers `connections describe` and `connections revalidate` too. Every
  answer goes out in connectors' framing: a script's bare success becomes
  `{"ok":true,"result":…}` on stdout, a script's `{"error":…}` becomes
  `{"ok":false,"error":{"code":"failure","data":…}}` on stderr with stdout empty, and `raw` is
  printed as given. Before adversary pass 2 it printed bare answers, and the transport read
  them; that shape is not connectors', and is no longer read.
- A loopback `TcpListener` in `tests/transport.rs` that answers one probe and returns the request
  it received.

## Acceptance and cases

`crates/llm-runpod/tests/transport.rs` holds 27 cases, `tests/adversary_w05_transport.rs` the 6 of the
first adversary pass and `tests/adversary_w05_transport_pass2.rs` the 4 of the second: 37.

| Acceptance | Cases |
| --- | --- |
| 1: create, list, get and terminate invoke `pod.create`, `pods.list`, `pods.list` with `id`, `pod.terminate`; each write carries a proof for its exact input | `create_prepares_issues_and_invokes_pod_create_with_a_proof_for_its_exact_input`, `list_invokes_pods_list_and_reads_every_pod`, `get_is_pods_list_with_the_id_filter_and_reads_last_started_at`, `terminate_invokes_pod_terminate_with_a_proof_for_the_pod_id`, `the_create_body_maps_every_request_field_to_a_declared_key` (`dockerEntrypoint` is the argv, `dockerStartCmd` is `[]`), `the_body_key_constant_is_the_specified_pod_create_body` |
| 2: `Refused` only for `refused`; `unknown`, a timeout, an unparseable answer are `Lost`; the GPU fallback both ways | `a_refused_create_is_refused`, `every_create_answer_that_is_not_a_definite_refusal_is_lost` (timeout, non-zero exit with no answer, unparseable, `not_attempted`, `applied` without a pod, no classification), `an_unknown_create_is_lost_after_one_listing_by_name_and_never_retried`, `an_unknown_create_that_the_listing_finds_is_created`, `an_unknown_create_through_the_provider_submits_no_second_create`, `a_refused_create_through_the_provider_tries_the_next_gpu` |
| 3: `complete` only for a whole listing | `a_listing_that_is_not_whole_is_never_complete` (truncated, throttled, interrupted, killed, not an array, one unreadable pod, an unknown status) |
| 4: the probe sends the vLLM key as a bearer to `<endpoint>models`; no file; `Debug` prints no key | `the_probe_sends_the_models_vllm_key_as_a_bearer_to_endpoint_models`, `the_probe_maps_each_answer`, `the_transport_debug_prints_no_key` |
| 5: crash-loop detection reads `lastStartedAt` moving forward | `last_started_at_parses_runpods_utc_timestamps`, `a_last_started_at_moving_forward_counts_as_a_restart` |
| adversary pass 1 | `a_cli_whose_stdout_outlives_it_does_not_outlive_the_timeout` (the process group is killed at the deadline), `an_unknown_create_is_not_resolved_to_a_pod_of_another_request` and `an_unknown_create_through_the_provider_does_not_accept_an_earlier_requests_pod` (an `unknown` create is `Created` only for a pod carrying its own `B10X_LLM_REQUEST`), `the_create_body_asks_for_no_pod_volume_as_llmgw_does`, `terminate_maps_404_to_refused_and_every_uncertain_answer_to_lost`, `the_proof_output_is_absolute_for_a_relative_work_directory` |
| expired connection evidence (connectors `docs/local-catalog-provider.md:479-486`) | `a_read_refused_at_admission_revalidates_the_connection_once_and_is_repeated_once`, `a_second_refusal_at_admission_is_not_repeated_again`, `a_create_refused_at_admission_before_dispatch_is_repeated_once_with_a_fresh_proof`, `a_write_refused_after_admission_or_classified_otherwise_is_never_repeated` |
| connectors' output framing (`contracts/cli/v1alpha1/semantics.md:178-181`): success on stdout, failure on stderr | `an_answer_outside_connectors_framing_is_not_read`, `a_failure_on_stderr_reaches_no_debug_output`; adversary pass 2: `a_listing_in_connectors_success_envelope_is_read`, `an_applied_create_in_connectors_success_envelope_is_created`, `a_refused_create_reported_on_stderr_is_refused`, `a_read_refused_at_admission_on_stderr_revalidates_once` |
| a stale descriptor: `stale_description`, `lifecycle_conflict` at `admission` (`semantics.md:579-581`, `:601`, `:606`) | `a_stale_description_refreshes_the_descriptor_once_and_repeats_the_read_once`, `a_lifecycle_conflict_at_admission_refreshes_revalidates_and_repeats_a_create_once`, `a_stale_descriptor_refused_twice_or_after_dispatch_is_not_repeated_again` |

## The planted inversion

With `crates/llm-runpod/src/transport/connectors.rs` changed so that `unknown` maps to
`CreateAnswer::Refused` (`Some("refused" | "unknown") => CreateAnswer::Refused`), the transport
lane at `25f61ee` went 18 passed → 15 passed, 3 failed:

```
test an_unknown_create_that_the_listing_finds_is_created ... FAILED
test an_unknown_create_is_lost_after_one_listing_by_name_and_never_retried ... FAILED
test an_unknown_create_through_the_provider_submits_no_second_create ... FAILED
test result: FAILED. 15 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
```

The provider-level case failed on its dispatch: the inverted transport let `RunpodProvider::create`
fall through to the second GPU, so the create ended `Rejected` instead of `Unknown`. The change
was reverted.

## What this does not establish

- That the real connectors CLI refuses an expired connection exactly as the fixture does, or
  refuses a write that way at all: the document names reads.
- That connectors v0.36.0 prints exactly these shapes: the fixture was written from the
  documents and ESS types above, not from a run of the real CLI.
- That a create succeeds: its body always carries `dockerStartCmd: []` (llmgw parity; the
  pinned `PodCreateInput` says "If [], uses the start CMD defined in the image") and, for a
  declared model, `dockerEntrypoint` (the vLLM argv, as llmgw `src/runpod.rs:781-799` at
  `048ebd8`), and `networkVolumeId` for a cached one; v0.36.0 refuses all three keys. A connectors release
  admitting them is requested upstream.
- That `lastStartedAt` moves when a container restarts, that Runpod returns a pod's `env` in a
  listing, or that a live pod's `https://` endpoint is reachable: the probe speaks plain HTTP only
  (story:pod-proxy-tls).

## Mutations planted for the second adversary pass

The framing and stale-descriptor cases were written after the code that satisfies them, so each
was shown live by a planted mutation, run, and reverted:

| Mutation in `src/transport/connectors.rs` | Result |
| --- | --- |
| `stale_description` and `lifecycle_conflict` no longer recovered | `transport.rs` 24 passed, 3 failed: the three stale-descriptor cases |
| stderr read but not parsed (`Answer::framed(status.code(), printed, None)`) | 13 failed across the three lanes, every case whose answer is a failure on stderr |
