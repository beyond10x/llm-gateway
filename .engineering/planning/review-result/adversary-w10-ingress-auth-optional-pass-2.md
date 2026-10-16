---
format: aep.planning-md/3
id: review-result:adversary-w10-ingress-auth-optional-pass-2
kind: review-result
status: active
title: Wave 2026-10-16-w01 adversary, story:ingress-auth-optional, pass 2
relations:
- reviews: story:ingress-auth-optional
revision: 1
---
unit: story:ingress-auth-optional, tree lg-w10-adv-ingress at a9ff8c3 (adv2/ingress-auth-optional) plus one untracked test file
verdict: green
cases: executed 218→223, red 0
origin: introduced 3, pre-existing 0, undecided 0
wrote-outside-worktree: none
needs-coordinator: no

**1. Diff proof**

`git --no-pager diff --stat` is empty because no tracked file changed. `git status --short` lists one new file, and it is a test file. No implementation file was touched:
```
?? crates/llm-gateway/tests/adversary_w10_ingress_pass2.rs
```

**2. Cases added** (all in `crates/llm-gateway/tests/adversary_w10_ingress_pass2.rs` of the tree)

All five pass on a9ff8c3. Each one fails on one mutant of the fix that the existing suite does not catch. Each case sends two copies of the owner's own token, so a parser that let the repeat through would authenticate the first copy.

| Case | What it asserts | Now | Mutant that turns it red |
|---|---|---|---|
| `adv2_none_a_repeated_owner_token_off_the_wire_paths_stays_credential_malformed` | Under `none`, with a secret or `NoOwnerVerifier`, 11 non-wire targets answer 401 `credential-malformed`. Covers `/metrics`, `/v1/routes`, `/v1/routes/open`, unknown paths, a trailing slash, `/V1/...`, a sub-path, `POST /v1/models`, `POST /` and a `?x=/v1/messages` query, each with lowercase and mixed-case names | green | M1: the parser callback drops the path check |
| `adv2_bearer_a_repeated_owner_token_on_a_wire_path_stays_credential_malformed` | Under `bearer`, all three wires answer 401 `credential-malformed` and the pod receives nothing | green | M3: the parser callback drops the `ingress_auth` check |
| `adv2_none_without_a_relay_a_repeated_owner_token_on_a_wire_path_stays_credential_malformed` | Under `none` with `Gateway::bind` (no relay), wire paths answer 401 `credential-malformed`. The mutant answers 405 or `path-unknown` instead | green | M2: the parser callback drops the relay check |
| `adv2_none_repeated_case_variant_and_malformed_authorizations_on_a_wire_path_are_ignored` | Four `authorization` lines (`Authorization`, `AUTHORIZATION`, an empty one, `Basic`) plus query strings give 200 on all three wires. GET, HEAD and PUT give 405. The pod's 6 heads contain no `authorization` and none of the values | green | none; this is the positive side |
| `adv2_none_a_repeated_authorization_does_not_carry_another_repeated_header_through` | A repeated `x-note`, `content-type` or `content-length` beside a repeated `authorization` is still 400 `request-malformed`, in either order | green | M4: `name == "authorization" &&` is dropped |

No case was red, so there is no red output. Running the new cases alone (`cargo test --locked -p b10x-llm-gateway --test adversary_w10_ingress_pass2`) gave EXIT=0 with 5 passed. Log: `target/lg-w10-adv/pass2/cases-alone.log`.

Mutant probe (`target/lg-w10-adv/pass2/mutate.sh`): it copies the crate to `target/lg-w10-adv/pass2/mut/` and mutates `server.rs` in that copy only. In the unmutated control everything is green except `dependency_boundary`, which fails only because the copy is outside the workspace. Under each of M1 to M4, the only failures are the matching case above and that same `dependency_boundary` artefact. The crate's 218 existing cases stay green under all four mutants. Grep found no repeated-authorization case in the scenarios or the CLI tests that would catch them either: the one existing scenario pins `/v1/routes` under `bearer` only. Logs: `target/lg-w10-adv/pass2/m{1..4}-*.log`.

**3. Suite run** (after the cases existed)
- `RUSTUP_TOOLCHAIN=1.98.0 RUSTC_WRAPPER=sccache cargo test --locked --no-fail-fast -p b10x-llm-gateway` gave EXIT=0: 223 passed, 0 failed. Log: `target/lg-w10-adv/pass2/suite-after.log`.
- For the before count I ran the same command again with `--lib` and every `--test` target except `adversary_w10_ingress_pass2` (there are 0 doc tests). It gave EXIT=0, 218 passed. Log: `target/lg-w10-adv/pass2/suite-before.log`.

**4. Findings** (tree a9ff8c3 plus the new test file)

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| 1 | `crates/llm-gateway/src/server.rs:607` | CONFIRMED / introduced | The parser callback has three conditions (`ingress_auth`, relay, wire path), and no existing test checks any of them. Dropping any one (M1, M2, M3) leaves all 218 cases green. M3 would make `bearer` accept an ambiguous credential on the wire paths when the first copy is the owner's. | Nothing reaches it at runtime today; all three conditions hold. It is reached by the next edit of `reads_no_credential` or `dispatch`. Fix: keep cases 1 to 3. |
| 2 | `crates/llm-gateway/src/server.rs:562` | CONFIRMED / introduced | Removing `name == "authorization" &&` (M4) lets every repeated header through on open wire paths, including two different `content-length` values, and all 218 cases stay green. | Same as #1: the guard holds today, and only a future edit would break it. Fix: keep case 5. |
| 3 | `spec/domains/gateway.yaml:102` | CONFIRMED / introduced | The `IngressAuth` comment cites 2 scenarios. It does not cite the new `ingress-none-with-an-owner-secret-the-owner-surface-takes-that-secret`, which is the only scenario covering a repeated authorization and none-with-a-secret. | Readers of the spec |

**5. Attacked and could not break**
- A repeated authorization on non-wire paths under `none` stays `credential-malformed`, with or without a secret.
- A repeated authorization on wire paths under `bearer` stays `credential-malformed`.
- Under `none`, a gateway bound without a relay keeps refusing a repeated authorization.
- Header-name case variants are lowercased before the duplicate check.
- A repeated authorization next to a malformed or empty one is admitted, and no client value reaches the pod.
- A query string is stripped before the parser decides whether a path is open, consistently with `decide`.
- Other repeated headers are still refused on open paths.
- The new binary test `hosted.rs::ingress_none_with_a_secret_serves_the_owner_surface_to_that_secret_only` would fail if `none` were ignored or if the secret were dropped.
- The new scenario really sends its `headers` (`checks/conformance/src/gateway.rs:1614`).
- The line ranges the matrix cites (`server.rs:598-604`, `651-655`) land on the cited code.
- `Admitted` holds no client header, so a client `authorization` cannot be forwarded.
- A non-UTF-8 or obs-folded `authorization` gets 400 `request-malformed`. This is the general rule for every head (concept page step 1), not a credential read.

**6. Paths written outside the worktree:** none. Everything I wrote is under `target/lg-w10-adv/pass2/`, including the mutant copy `mut/` (157M with its own build directory), which the coordinator can delete. Lease `lg-w10-adv-ingress-2` is released.

```findings
[
  {"file": "crates/llm-gateway/src/server.rs", "line": 607, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "no existing test checks any of the three conditions the parser callback applies (ingress_auth, relay, wire path), so dropping any one keeps all 218 cases green, including one that makes bearer accept a repeated owner credential on wire paths"},
  {"file": "crates/llm-gateway/src/server.rs", "line": 562, "category": "mutant", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "dropping the authorization-name guard lets every repeated header, conflicting content-length included, through on open wire paths with the suite still green"},
  {"file": "spec/domains/gateway.yaml", "line": 102, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the IngressAuth comment cites two scenarios and omits the new one that covers a repeated authorization and none with an owner secret"}
]
```
