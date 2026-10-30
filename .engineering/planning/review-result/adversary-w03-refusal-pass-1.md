---
format: aep.planning-md/3
id: review-result:adversary-w03-refusal-pass-1
kind: review-result
status: active
title: Wave 2026-10-30-w03 adversary pass, refusal texts, pass 1
relations:
- reviews: story:trusted-refusal-names-no-path
- reviews: story:translation-refusal-names-path
revision: 1
---
## Scope

Wave 2026-10-30-w03 adversary pass 1 over story:trusted-refusal-names-no-path (f431abc) and
story:translation-refusal-names-path (357c1a4), at 6c7b90d. Added
`crates/llm-gateway-cli/tests/adversary_w03_refusal_paths.rs`, 4 cases through the binary: 3 red,
1 green. Suite: 179 cases, 175 green without the added file.

## Red cases

- `adv_w03_credential_set_refusal_does_not_print_the_credential_file_path`: `refused
  credential:directory-missing: <dir>/sk-adv-path-marker-5e1b: No such file or directory`.
- `adv_w03_credential_set_success_does_not_print_the_credential_file_path`: `credential stored
  <dir>/sk-adv-path-marker-5e1b/key probe skipped`.
- `adv_w03_a_secret_shaped_key_is_not_copied_into_the_refusal_log_line`: a 64-byte key of
  `[A-Za-z0-9_-]` the client sent is in the `translation refused` line, as llm's `field_path`
  copies it (llm-core controls.rs:355-375).

## Held

Every trusted-file read and caller is free of the path, including canonicalize and errno texts;
the `deployment loaded` debug line logs only the digest; every llm `invalid`, `unsupported` and
`too_large` reachable from decode, ingress_provenance, project, validate_for and check_request is
literal or interpolates a static name, the wire, `field_path`/`path` or a fixed list; every
translation refusal goes through `refused()` once; a Bedrock route writes one line per refusal.

```findings
- file: CHANGELOG.md
  line: 11
  category: contract-drift
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the Breaking entry says a secret written into a *_file setting is not printed or logged, but credential set still prints that setting's path in refusals and on success
- file: crates/llm-gateway-cli/src/credential.rs
  line: 457
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: credential set's credential:directory-* and credential:write refusals print the *_file path or its directory (adv_w03_credential_set_refusal_does_not_print_the_credential_file_path is red)
- file: crates/llm-gateway-cli/src/credential.rs
  line: 565
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: the documented success line "credential stored <path>" prints the *_file value, so a key written there by mistake is printed (adv_w03_credential_set_success_does_not_print_the_credential_file_path is red)
- file: crates/llm-gateway-cli/src/translate.rs
  line: 378
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: a client key of 64 bytes of [A-Za-z0-9_-] shaped as a secret is logged verbatim in the translation-refused line; it matches the stated design and no client was found that sends a secret as a JSON key
```
