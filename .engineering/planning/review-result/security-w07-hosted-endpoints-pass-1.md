---
format: aep.planning-md/3
id: review-result:security-w07-hosted-endpoints-pass-1
kind: review-result
status: active
title: Wave 2026-10-14-w01 security review, story:hosted-endpoints, pass 1
relations:
- reviews: story:hosted-endpoints
revision: 1
---
```
unit: story:hosted-endpoints, branch impl/hosted-endpoints at da32b6f plus the uncommitted implementation
verdict: NEEDS-CHANGE (one judgement finding, a warning; no red case)
cases: executed 0→7, red 0
origin: introduced 2 / pre-existing 0 / undecided 0
```

## Cases added

`crates/llm-gateway-cli/tests/security_w07_hosted.rs`, all green:
- `security_w07_client_credential_headers_never_reach_an_endpoint`: client `authorization`, `x-api-key`, `api-key`, `proxy-authorization`, `cookie` and `x-goog-api-key` never reach any endpoint; each endpoint receives only `host`, its own credential header and the framing headers.
- `security_w07_trace_logs_and_failure_answers_withhold_credential_and_url`: at `RUST_LOG=trace`, across answering, refusing, closing, 502 and untrusted-https endpoints, no answer or log line holds a credential, a credential path, a base URL or an authority.
- `security_w07_credential_header_cannot_name_a_reserved_header_or_inject_bytes`: all reserved names in mixed case, CR, LF, colon, space, NUL, non-ASCII, empty and 65-byte names are refused.
- `security_w07_no_document_key_reaches_a_trust_root_or_disables_verification`: ten trust-shaped keys are refused `config:schema`.
- `security_w07_a_non_loopback_http_endpoint_with_a_credential_is_admitted`: green by design; the evidence for finding 1.
- `security_w07_a_hosted_endpoint_debug_shows_no_credential`.
- `security_w07_credential_file_rules_match_the_owner_secret_rules`: 35 file shapes end the same way as `owner_secret_file` and as `credential_file`.

## Findings

1. Cleartext credentials (`crates/llm-gateway-cli/src/hosted.rs:95`, `spec/domains/deployment.yaml:362`): a bearer or api-key endpoint at a non-loopback `http://` base URL is admitted, and its credential is written over plain TCP with no explicit opt-in. Recommended: refuse it as `config:value`, or require an opt-in key; a spec change.
2. Echoed credentials (`crates/llm-gateway/src/relay.rs:900`): answer bodies are relayed unchanged, so an endpoint that echoes its credential in its body puts it in a client answer. Infeasible to sanitise; document that the gateway does not rewrite answer bodies.

## Reviewed and could not fault

The request head is the gateway's own and forwards no client header; the reserved list covers every header the gateway writes; the endpoint credential follows the owner-secret trusted-file rules; the https handshake and verification complete inside `connect` before any byte, with only `webpki-roots` trusted; rustls is built without its `logging` feature; the base URL check refuses userinfo, paths, `%`, `\`, CR/LF and over-long authorities; the inventory names only the alias, `hosted` and `upstream_model`.

```findings
- file: crates/llm-gateway-cli/src/hosted.rs
  line: 95
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: a bearer or api-key hosted endpoint at a non-loopback http:// base URL is admitted and sent its credential in cleartext with no explicit opt-in; the spec should refuse it as config:value or require an opt-in key
- file: crates/llm-gateway/src/relay.rs
  line: 900
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: answer bodies are relayed unchanged, so an endpoint that echoes its credential in an error body puts it in a client response; document that the gateway does not rewrite answer bodies
```
