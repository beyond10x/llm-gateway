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
  as `proof_mismatch`.
- A loopback `TcpListener` in `tests/transport.rs` that answers one probe and returns the request
  it received.

## Acceptance and cases

All in `crates/llm-runpod/tests/transport.rs`: 18 cases.

| Acceptance | Cases |
| --- | --- |
| 1: create, list, get and terminate invoke `pod.create`, `pods.list`, `pods.list` with `id`, `pod.terminate`; each write carries a proof for its exact input | `create_prepares_issues_and_invokes_pod_create_with_a_proof_for_its_exact_input`, `list_invokes_pods_list_and_reads_every_pod`, `get_is_pods_list_with_the_id_filter_and_reads_last_started_at`, `terminate_invokes_pod_terminate_with_a_proof_for_the_pod_id`, `the_create_body_maps_every_request_field_to_a_declared_key` (`dockerEntrypoint` is the argv, `dockerStartCmd` is `[]`), `the_body_key_constant_is_the_specified_pod_create_body` |
| 2: `Refused` only for `refused`; `unknown`, a timeout, an unparseable answer are `Lost`; the GPU fallback both ways | `a_refused_create_is_refused`, `every_create_answer_that_is_not_a_definite_refusal_is_lost` (timeout, non-zero exit with no answer, unparseable, `not_attempted`, `applied` without a pod, no classification), `an_unknown_create_is_lost_after_one_listing_by_name_and_never_retried`, `an_unknown_create_that_the_listing_finds_is_created`, `an_unknown_create_through_the_provider_submits_no_second_create`, `a_refused_create_through_the_provider_tries_the_next_gpu` |
| 3: `complete` only for a whole listing | `a_listing_that_is_not_whole_is_never_complete` (truncated, throttled, interrupted, killed, not an array, one unreadable pod, an unknown status) |
| 4: the probe sends the vLLM key as a bearer to `<endpoint>models`; no file; `Debug` prints no key | `the_probe_sends_the_models_vllm_key_as_a_bearer_to_endpoint_models`, `the_probe_maps_each_answer`, `the_transport_debug_prints_no_key` |
| 5: crash-loop detection reads `lastStartedAt` moving forward | `last_started_at_parses_runpods_utc_timestamps`, `a_last_started_at_moving_forward_counts_as_a_restart` |

## The planted inversion

With `crates/llm-runpod/src/transport/connectors.rs` changed so that `unknown` maps to
`CreateAnswer::Refused` (`Some("refused" | "unknown") => CreateAnswer::Refused`), the transport
lane went 18 passed → 15 passed, 3 failed:

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

- That connectors v0.36.0 prints exactly these shapes: the fixture was written from the
  documents and ESS types above, not from a run of the real CLI.
- That a create succeeds: its body always carries `dockerStartCmd: []` and, for a declared
  model, `dockerEntrypoint` (the vLLM argv, as llmgw `src/runpod.rs:792-793` at `048ebd8`), and
  `networkVolumeId` for a cached one; v0.36.0 refuses all three keys. A connectors release
  admitting them is requested upstream.
- That `lastStartedAt` moves when a container restarts, that Runpod returns a pod's `env` in a
  listing, or that a live pod's `https://` endpoint is reachable: the probe speaks plain HTTP only
  (story:pod-proxy-tls).
