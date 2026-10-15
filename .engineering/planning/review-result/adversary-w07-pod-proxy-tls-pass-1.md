---
format: aep.planning-md/3
id: review-result:adversary-w07-pod-proxy-tls-pass-1
kind: review-result
status: active
title: Wave 2026-10-14-w01 adversary, story:pod-proxy-tls, pass 1
relations:
- reviews: story:pod-proxy-tls
revision: 1
---
```
unit: story:pod-proxy-tls, impl/pod-proxy-tls c6b31e7..fd95b58; findings cover fd95b58 (cases committed on top as 1ec6278 guards, 001d3b0 red)
verdict: NEEDS-CHANGE
cases: executed 142→150, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
```

## Cases added

`crates/llm-gateway-cli/tests/adversary_w07_pod_tls_red.rs` (red):
- `adversary_w07_a_silent_handshake_gives_up_within_the_connect_bound`: a loopback endpoint that accepts TCP and never answers the ClientHello; the connect was still waiting after 15 s, against the 10 s connect bound.
- `adversary_w07_the_documented_release_build_builds_no_seam_program`: the documented release build builds `gateway-tls-seam`, which trusts a root from its environment.

`crates/llm-gateway-cli/tests/adversary_w07_pod_tls.rs` (guards, green):
- `adversary_w07_every_bad_certificate_fails_both_connections_and_the_pod_reads_nothing`
- `adversary_w07_a_plain_http_answer_on_the_https_port_opens_no_connection`
- `adversary_w07_an_untrusted_pod_is_probed_without_a_byte_or_a_key`
- `adversary_w07_the_shipped_start_refuses_a_bad_connectors_document_before_any_call`
- `adversary_w07_the_shipped_binary_links_no_seam_and_reads_no_certificate_variable`
- `adversary_w07_no_tls_crate_in_the_gateway_or_runpod_closure`

## Findings

1. The handshake is not bounded by the 10 s connect timeout (`crates/llm-gateway-cli/src/tls.rs:133`): `open` sets the read and write timeouts to the 120 s I/O bound before `complete_io`, so an endpoint that accepts TCP and stalls holds the connect past the 10 s the pod endpoint rule promises.
2. The documented build produces the seam program (`crates/llm-gateway-cli/Cargo.toml:59`): `gateway-tls-seam` has no `required-features`, so `cargo build --release --locked -p b10x-llm-gateway-cli` writes it beside the shipped binary. The Dockerfile installs only `b10x-llm-gateway`, and the shipped binary links no seam code.
3. The boundary test cannot catch a TLS crate (`crates/llm-gateway/tests/dependency_boundary.rs:22`): its forbidden list names none; the closures are clean today, held by the guard `adversary_w07_no_tls_crate_in_the_gateway_or_runpod_closure`.

## Attacked and could not break

Wrong host or SNI, expired or not yet valid, self-signed, a non-webpki root, a non-CA intermediate, a CN-only name, a too-wide wildcard and a client-auth-only EKU are refused before any byte on the relay and the probe paths; a plain-HTTP answer on the https port is refused; the probe sends no key to an untrusted pod; the shipped binary links no seam code and reads no certificate variable; every document refusal precedes the first connectors call and quotes no key; neither `b10x-llm-gateway` nor `b10x-llm-runpod` gained a dependency.

```findings
- file: crates/llm-gateway-cli/src/tls.rs
  line: 133
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the relay's TLS handshake reads under the 120 s I/O timeout, so a pod endpoint that accepts TCP and stalls holds the connect past the 10 s the pod endpoint rule promises (adversary_w07_a_silent_handshake_gives_up_within_the_connect_bound)
- file: crates/llm-gateway-cli/Cargo.toml
  line: 59
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: gateway-tls-seam has no required-features, so the documented release build places a program that trusts an environment-named root beside the shipped binary (adversary_w07_the_documented_release_build_builds_no_seam_program)
- file: crates/llm-gateway/tests/dependency_boundary.rs
  line: 22
  category: acceptance
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: acceptance 4 rests on a FORBIDDEN list naming no TLS crate, so the boundary test would pass with rustls or ring in the gateway closure; only adversary_w07_no_tls_crate_in_the_gateway_or_runpod_closure checks it
```
