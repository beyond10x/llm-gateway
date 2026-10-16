---
format: aep.planning-md/3
id: review-result:adversary-w10-ingress-auth-optional-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w01 adversary, story:ingress-auth-optional, pass 1
relations:
- reviews: story:ingress-auth-optional
revision: 1
---
unit: story:ingress-auth-optional, tree lg-w10-adv-ingress at 1f500d4 (impl/ingress-auth-optional) plus one untracked test file
verdict: red
cases: executed 209→215, red 2
origin: introduced 5, pre-existing 0, undecided 0
wrote-outside-worktree: none
needs-coordinator: no

**1. Diff proof**
`git --no-pager diff --stat` is empty, because nothing tracked changed. `git status --short` shows one new file, a test file, and no implementation file was touched:
```
?? crates/llm-gateway/tests/adversary_w10_ingress.rs
```

**2. Cases added** (all in `crates/llm-gateway/tests/adversary_w10_ingress.rs` of the tree)

| Case | Asserts | Now |
|---|---|---|
| `adv_none_a_repeated_authorization_on_a_wire_path_is_no_refusal` | With `none`, a POST to chat carrying two `authorization` headers is relayed (200) | red |
| `adv_the_concept_pages_decision_order_is_one_unbroken_list` | Steps 1 to 6 of "The order a request is decided in" sit in one section | red |
| `adv_none_with_a_secret_serves_the_owner_surface_to_that_secret_only` | Acceptance 2, second half: `/metrics` and `/v1/routes` answer 200 to the owner, `credential-absent` with no header, `credential-rejected` to a guess | green |
| `adv_none_opens_only_the_three_literal_wire_paths_of_a_relaying_gateway` | A gateway with no relay keeps the wire paths at 401; case, trailing slash, `//`, `%00`, sub-paths, routes and unknown paths stay 401 for both POST and GET | green |
| `adv_none_without_an_owner_is_indistinguishable_from_one_with_a_secret` | `NoOwnerVerifier` answers the same as `SharedSecretVerifier` for 6 method/path pairs × 5 header shapes | green |
| `adv_none_records_an_unauthenticated_call_and_forwards_no_client_credential` | 3 relayed records plus 1 record for a 405; no `authorization`, `x-api-key` or owner token reaches the pod, query string included | green |

Red output, each case run alone before the suite:
```
assertion `left == right` failed: pod saw []
  left: (401, "credential-malformed")
 right: (200, "")
```
```
step 6 is not in the section:
The order a request is decided in
1. ... 5. **Authentication of everything else ...
```

**3. Suite run** (after the cases existed)
- `RUSTUP_TOOLCHAIN=1.98.0 RUSTC_WRAPPER=sccache cargo test --locked --no-fail-fast -p b10x-llm-gateway` gave EXIT=101: 213 passed, 2 failed (`adversary_w10_ingress`), 215 total. Log: `target/lg-w10-adv/suite-after.log`.
- For the before count I ran the same command again with every test target except `adversary_w10_ingress`. It gave EXIT=0, 209 passed. Log: `target/lg-w10-adv/suite-before.log`.

**4. Findings** (tree 1f500d4 plus the new test file)

| # | file:line | Verdict / origin | What was measured | What reaches it |
|---|---|---|---|---|
| 1 | `crates/llm-gateway/src/server.rs:557` | NEEDS-CHANGE / introduced | With `none`, two `authorization` headers to a wire path get 401 `credential-malformed` from the head parser before `decide` runs. The story says "no `authorization` header is read", and `docs/gateway.md:189` says "a malformed or wrong credential is no refusal". | A mesh sidecar that adds its own `authorization` next to the client's. Fix: under `none`, skip the duplicate-`authorization` refusal for wire paths and move it into `authenticate`, or narrow the docs. |
| 2 | `website/docs/concepts/single-owner-gateway.md:51` | NEEDS-CHANGE / introduced | The new section was inserted inside the numbered list, so step 6 (`:75`) now renders under "Model calls without a credential". | Every reader of the public site. Fix: move the section after step 6. |
| 3 | `crates/llm-gateway-cli/tests/hosted.rs:543` | CONFIRMED / introduced | Acceptance 2 asks for a scenario and a binary test of "`none` with an owner secret is served to that secret only". Neither exists: the scenarios never show an owner being served, and `binary.rs` and `tests/gateway.rs` are unchanged, though the story scope names them. Case 3 above covers it at library level. | The acceptance statement |
| 4 | `crates/llm-gateway/src/metrics.rs:32` | CONFIRMED / introduced | The doc says a record is made only for a request "that passed owner authentication". With `none` an unauthenticated call is recorded too (case 6). `server.rs:377` has the same wording. | Anyone implementing `UsageRecords` |
| 5 | `crates/llm-gateway/src/auth.rs:316` | CONFIRMED / introduced | The docs say `credential-rejected` when a header is sent, but `Basic …` or a bare `Bearer` gets `credential-malformed`. This is the same as a gateway that has a secret (case 5), so only the wording is wrong. The same sentence is at `docs/gateway.md:194`, `spec/domains/deployment.yaml:455` and the concept page `:65`. | Readers of the docs |

**5. Attacked and could not break**
- The wire paths do not open on a gateway composed without a relay.
- Path variants (case, trailing slash, `//`, `%00`, sub-paths) stay at 401.
- A query string on a wire path is admitted and is not forwarded.
- GET on a wire path gives 405 and makes a usage record.
- No client `authorization` or `x-api-key` reaches the target, the owner's own token included.
- From outside you can't tell whether the gateway has an owner secret.
- `bearer` without `owner_secret_file` is refused `config:value`.
- Other TOML values for `ingress_auth` are refused `config:schema`.
- The warning line comes before `listening on` (unit's test in `hosted.rs`).

**6. Paths written outside the worktree:** none. The logs are inside the tree under `target/lg-w10-adv/`, and the lease is released.

```findings
- file: crates/llm-gateway/src/server.rs
  line: 557
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: under ingress_auth none a wire-path request with a repeated authorization header is refused 401 credential-malformed, contradicting "no authorization header is read" and "a malformed credential is no refusal"
- file: website/docs/concepts/single-owner-gateway.md
  line: 51
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the new section is inserted inside the numbered decision-order list, orphaning step 6 under the wrong heading on the public site
- file: crates/llm-gateway-cli/tests/hosted.rs
  line: 543
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: acceptance 2's "with none and an owner secret the owner surface is served to that secret only" has no scenario or binary test as the criterion requires
- file: crates/llm-gateway/src/metrics.rs
  line: 32
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: UsageRecord and server.rs record() say records are made only for owner-authenticated wire requests, but under ingress_auth none unauthenticated calls are recorded
- file: crates/llm-gateway/src/auth.rs
  line: 316
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: NoOwnerVerifier docs promise credential-rejected for any presented header, but a non-Bearer or empty-token header is credential-malformed, as with a secret
```
