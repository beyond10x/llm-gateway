# Runpod hosting verification — 2026-09-26

The [Runpod section of the hosting contract](../hosting.md#runpod) describes the adapter and its
limits. Every case here drives `EmulatedRunpod`, an in-process transport; nothing opens a socket,
reads a credential or allocates a cloud resource.

## Commands

```sh
cargo test -p b10x-llm-runpod --locked
cargo clippy -p b10x-llm-runpod --all-targets --all-features --locked -- -D warnings
cargo fmt -p b10x-llm-runpod --check
```

`crates/llm-runpod/tests/runpod.rs` holds 37 cases, `tests/adversary.rs` the six of the first
adversarial pass (unchanged apart from the `crash_window_ms` field in its fixture),
`tests/adversary2.rs` the three of the second, `tests/round2.rs` two, and `src/` four unit
cases: 52 in all. The
acceptance maps onto them as follows.

| Acceptance clause | Cases |
| --- | --- |
| single-flight startup | `concurrent_first_requests_create_exactly_one_pod`, `a_request_while_the_pod_starts_waits_on_it_instead_of_creating_another`, `an_unresolved_lost_create_blocks_a_second_pod_until_it_is_resolved`, `a_lost_create_answer_is_never_retried_on_another_gpu_and_is_adopted_by_request_id` |
| readiness and crash recovery | `a_crash_looping_pod_is_terminated_and_the_next_request_starts_a_fresh_one`, `a_pod_that_refuses_its_credential_is_terminated`, `a_pod_that_misses_its_startup_deadline_is_terminated`, `a_pod_that_vanishes_out_of_band_is_replaced_on_the_next_request`, `a_partial_listing_neither_discharges_nor_replaces_a_running_pod`, `an_unreported_readiness_or_served_model_stays_unknown` |
| ownership-safe adoption | `a_restarted_pool_adopts_its_own_pod_by_exact_identity_without_creating`, `a_restarted_pool_does_not_adopt_a_pod_that_reused_its_name`, `a_pod_retagged_by_a_newer_owner_is_handed_over_and_never_terminated` |
| reachable idle and orphan cleanup | `an_idle_pod_is_reaped_and_the_next_request_cold_starts_again`, `an_open_stream_lease_keeps_the_pod_from_being_reaped`, `the_idle_limit_is_never_below_the_measured_cold_start`, `the_orphan_sweep_terminates_only_this_controllers_unrecorded_pods`, `the_orphan_sweep_never_selects_a_legacy_llmgw_pod`, `a_model_removed_from_the_registry_has_its_pod_stopped_after_restart` |
| declared vLLM settings | `the_create_request_carries_the_declared_vllm_settings`, `a_model_without_a_volume_carries_no_placement_or_cache_override`, `reasoning_effort_and_explicit_sampling_reach_the_entrypoint`, `a_placement_refusal_falls_through_to_the_next_declared_gpu_in_order`, `every_declared_gpu_refusing_withdraws_the_deployment_and_leaves_no_pod`, `a_model_declaration_without_a_usable_setting_is_refused` |

## Falsification

`runpod-falsification.json` records 39 deliberate defects: the 29 of earlier rounds whose code
still exists, and 10 aimed at the second round's code (F1, F4 and J1 re-aimed at the rewritten
request id and restart count; F7, F8, F9, J5, J6 new). one per guarded behaviour. Each was
applied alone, the crate suite run, and the source restored byte for byte (`source_before_sha256`
equals `source_restored_sha256` in every entry). Every defect failed its named case.

## Correction round 1

| Finding | Fix | Class, and its members |
| --- | --- | --- |
| F1 request id shared across controllers | `request-<controller>-<alias>-<generation>-<instant>-<nonce>` | identifiers written to Runpod that another create could reproduce: the request id (fixed); the pod name (shared on purpose, never an identity); the owner tag (the controller's own identity); the epoch (read only together with the owner) |
| F2 a request id protects a keyed record's duplicate | request ids protect only records with no key yet | sweep protection weaker than the exact key: the key (exact); the request id (now keyless records only) |
| F3 `EXITED` read as terminated | `EXITED` is present-but-not-running, retired as `pod-exited` and terminated | provider statuses that end an obligation: `CREATED` and `EXITED` do not, `RUNNING` does not, only `TERMINATED` does |
| F4–F6 surviving mutants | none; the adversary's cases kill them, and the zero-`crash_window_ms` clause got its own case | validation clauses refused one at a time: all twelve now each have a case |
| J1 lifelong restart count | restarts count inside `crash_window_ms`, end inclusive; the redundant prune in `poll` was deleted because it made the same decision and could never trip | — |
| J3 endpoint for any pod | only `RUNNING` reports one | — |
| J4 per-alias in-flight count | a new `Usage` per deployment | — |
| J2 orphan terminations bypass the ledger | not changed; described as open in the contract | — |

## Correction round 2

| Finding | Fix |
| --- | --- |
| F7 inherited pod parked in `ownership-lost` | the pool records the snapshot's previous lease holder per exact key and terminates a parked pod whose label still names exactly that holder; any other label is never touched (`a_takeover_never_terminates_a_pod_a_third_controller_has_retagged`) |
| F8 a pod that served and then stays not-ready | retired as `stopped-serving` once a definite "not ready" follows its last ready observation by more than the startup deadline; unknown readiness does not count |
| F9, J5 request id too long, ambiguous | readable prefix of at most 64 bytes plus a 128-bit digest of a length-prefixed encoding; at most 96 bytes |
| J6 restarts kept for a pod's whole life | pruned outside the window in `poll`; the count reads what is left, so the window is decided in one place |

## Limits

The `llm.runpod` ESS domain (`spec/domains/runpod.yaml`) drives the same pool and emulator through
`checks/conformance/src/runpod.rs`: 45 authored scenarios under `contracts/runpod/scenarios`, each
guarded behaviour with a falsification record in `runpod-ess-falsification.json`. No production
transport exists, and none of the
live control-plane assumptions listed in the contract has been checked against Runpod.
