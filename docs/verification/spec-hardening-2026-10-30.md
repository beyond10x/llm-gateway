# Specification hardening, 2026-10-30

This record covers the `ess:hardening` techniques run against `spec/` at llm-gateway `70841f2a`
(release 0.16.0 plus its image record), with ESS 0.77.0. It follows
[spec-hardening.md](spec-hardening.md) (2026-10-06, `a11ef83`, 179 scenarios) and looks at
what changed since: translation and the reasoning seal, the routes document, the metrics-only
listener, `ingress_auth = "none"`, spend ceilings, the cold-start hold, the max-tokens clamp and
the credential-namespace guide. The conformance suite was green before any technique ran:
`cargo run --locked -p b10x-llm-gateway-conformance -- check` passed 288 of 288 scenarios on
each of its three runs, with the projection drift check, `sources-sha256:18fa4b81…`. Every
technique saw its planted defect fail before its green result was counted.

## Results

| # | Technique | Planted defect and what it produced | Green result after restoring | Findings |
| --- | --- | --- | --- | --- |
| 8 | Design review, the deployment keys of `docs/gateway.md`, `docs/hosting.md`, README, the guides `run-in-a-credential-namespace.md`, `scrape-the-counters.md`, `run-on-kubernetes.md` and `deploy/kubernetes/credential-namespace/` against `spec/domains/deployment.yaml` | `metrics_listen` deleted from `DeploymentConfiguration` in a copy of the specification; reported as a document-only key, citing `website/docs/guides/scrape-the-counters.md:133` | the specification itself: 24 document keys, 79 declared fields, no key undeclared | none |
| 8 | Design review, the refusal table of `docs/gateway.md` ("Refusals") and its outcome table ("Call outcomes") against `llm-gateway.gateway.Refusal` | the same copy as above | 29 codes, each with the same status in the document and the specification; the outcome table names the same 29 codes | none |
| 8 | Design review, the counters, latency histogram, metrics-only listener, `ingress_auth`, spend ceilings and translated-path bounds of `docs/gateway.md` and `docs/hosting.md` against the `DECIDED` declarations of `telemetry.yaml`, `deployment.yaml` and `gateway.yaml` | the same copy as above | the series list, the 13 bucket bounds, the listener's path, method and token rules, the `none` owner-surface rules and the five translated-path bounds read the same in both | none; the rules the documents state that the specification declares only in comments stay with `epic:spec-hardening`, as in the 2026-10-06 record |
| 8 | Design review, the configuration document's field names in `crates/llm-gateway-cli/src/config.rs` against the six deployment declarations | not a separate plant; the comparison is the same field-name set | `Document`, `HostedDocument`, `ProviderDocument`, `ConnectorsDocument`, `ModelDocument` and `ProfileDocument` each name exactly the fields of their declaration (9, 19, 5, 5, 24, 6) | none |
| 7 | Spec diff, `ess verify diff --fail-on breaking-or-unknown` (ESS 0.77.0) | `Overloaded` removed from `llm-gateway.gateway.Refusal` in a copy: exit 4, `breaking type/llm-gateway.gateway.Refusal/variant-removed/Overloaded` (breaking for history), plus `system/llm-gateway/unclassified-changed` | the specification against itself: exit 0 | tag `0.15.0` to this revision: exit 0, "no semantic change". `a11ef83` to this revision: exit 4, 174 changes, 113 `compatible`, 61 `unknown`, 0 `breaking`: fields added to views, entities and the deployment types (unknown for history), five `ModelDeclaration` fields and `owner_secret_file` widened to `Optional`, and `wire` removed from `llm-gateway.upstream.HostedEndpoint`, an entity that was planned and unimplemented at `a11ef83`. None was acknowledged, because the gate is not in `task check` (`story:spec-diff-gate`, draft). A probe outside the plant: changing only `Overloaded`'s `status` attribute from 503 to 500 is reported as `system/llm-gateway/unclassified-changed` alone, not as a change to the variant |
| 1 | Mutation audit by hand, `crates/llm-runpod/src/spend.rs:182`, `projected > cap` to `>=` | 286 of 288: `a-start-exactly-at-the-daily-ceiling-is-created` and `a-start-exactly-at-the-total-ceiling-is-created` failed | restored; see the last row | killed |
| 1 | `crates/llm-runpod/src/provider.rs:158`, the running estimate's floor `.max(` to `.min(` | 286 of 288: `a-live-pods-running-estimate-counts-against-the-ceiling` and `a-pod-just-started-counts-its-startup-estimate` failed | restored | killed |
| 1 | `crates/llm-runpod/src/provider.rs:167`, the daily spend read from `now - now % DAY_MS` to `now` | 287 of 288: `spend-billed-before-the-utc-days-start-is-not-todays` failed | restored | killed |
| 1 | `crates/llm-gateway/src/body.rs:67`, the max-tokens clamp `value <= room` to `value <= room + 1` | 287 of 288: `clamp-a-max-tokens-above-the-room-left-is-lowered-to-it-and-one-at-or-below-is-unchanged` failed | restored | killed |
| 1 | `crates/llm-gateway/src/metrics.rs:233`, a histogram bucket `duration_ms <= bound` to `<` | 288 of 288, three runs: survived the suite. `cargo test -p b10x-llm-gateway`: `metrics::tests::the_published_buckets_are_the_histograms_and_each_flips_at_its_bound` failed | restored | survived the suite; the scenarios observe wall-clock durations (`duration_ms=bounded`), so none can sit on a bound |
| 1 | `crates/llm-gateway/src/server.rs:416`, the metrics-only listener's `request.path != METRICS ||` dropped | 288 of 288, three runs: survived the suite. `cargo test -p b10x-llm-gateway`: `the_metrics_listener_serves_the_counters_without_a_credential_and_nothing_else` failed | restored | survived the suite; no scenario reaches the `metrics_listen` listener (the suite's `Scrape` step is the owner's `/metrics`) |
| 1 | `crates/llm-gateway/src/server.rs:775`, `ingress_auth = "none"` reading no credential on wire paths only, `Wire::from_path(path).is_some()` to `!path.is_empty()` | 288 of 288, three runs: survived the suite. `cargo test -p b10x-llm-gateway`: `adv2_none_a_repeated_owner_token_off_the_wire_paths_stays_credential_malformed` failed | restored | survived the suite |
| 1 | `crates/llm-gateway-cli/src/seal.rs:94`, the seal's message taking the alias in place of the upstream model | 288 of 288, three runs: survived the suite. `cargo test -p b10x-llm-gateway-cli`: `seal::tests::seal_opens_on_its_own_scope_only` failed | restored | survived the suite; equivalent at the gateway's boundary today: a route relays to one target and the key lives for one process, so an alias names one upstream model for every seal it can open |
| 1 | `crates/llm-gateway-cli/src/translate.rs:1199`, the sealed reasoning item's summary condition negated | 288 of 288, three runs: survived the suite. `cargo test -p b10x-llm-gateway-cli`: `translation_codexs_two_turns_reach_a_messages_upstream_with_the_thinking_it_signed` failed | restored | survived the suite |
| 1 | After the last restore | not applicable | `check` 288 of 288 on three runs, `sources-sha256:18fa4b81…` as before; `cargo test -p b10x-llm-gateway -p b10x-llm-gateway-cli -p b10x-llm-runpod`: 775 passed, 0 failed | — |

Each mutant was one line, planted and restored with Git in the hardening tree; after each
restore no file under `crates/` differed from `70841f2a`.

## Findings

- The metrics-only listener (`telemetry.yaml:78`, `DECIDED 2026-10-17`) has no scenario: its
  path, method and scrape-token rules are pinned by crate tests only. It is the one listener that
  answers without the owner credential.
- Under `ingress_auth = "none"`, the owner surface's `credential-malformed` for anything but one
  bearer token (`deployment.yaml:710-711`) is pinned by an adversary test only.
- The first sealed reasoning item carries the streamed item's `id` and the reasoning shown as its
  summary (`gateway.yaml:80-81`); no scenario observes the summary.
- `SealScope.upstream_model` (`gateway.yaml:212`) cannot be observed apart from the alias while a
  route has one target; it becomes observable with `story:target-fallback`.
- The histogram's bucket bound is inclusive (`telemetry.yaml:66-69`); the conformance target has
  no clock seam, so a scenario cannot place a duration on a bound.
- `ess verify diff` reports a change to an enum variant's attribute (a refusal's `status`) only as
  `system/<name>/unclassified-changed`. The gate still fails on it as `unknown`, but the change
  is not named.

## Not run

- Technique 2 (random sequences): the compiled IR declares 0 guards, 0 transitions and 4
  outcomes, its 19 lifecycles have one state each, and the explorer ships only in ESS's Go and
  TypeScript packages; the implementation is Rust.
- Technique 4 (determinism): runs the technique-2 explorer twice; not available, as above.
- Technique 5 (metamorphic relations): needs a reference model to step through; the IR has no
  transition to step.
- Technique 6 (guard analysis): 0 guards and 0 invariants in the IR.
- `ess verify conform mutate`: mutates lifecycles, guards, `sets` and payloads, none of which the
  IR declares beyond the 4 outcomes (the 2026-10-06 pass found every mutant stillborn).
- Technique 3 (caller replay): no recorded caller trace was available.
