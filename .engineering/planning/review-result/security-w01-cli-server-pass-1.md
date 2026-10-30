---
format: aep.planning-md/3
id: review-result:security-w01-cli-server-pass-1
kind: review-result
status: active
title: Wave 2026-10-30-w01 security review, cli and server units, pass 1
relations:
- reviews: story:config-refusal-quotes-no-value
- reviews: story:serve-cli-echoes-no-argument
- reviews: story:ingress-auth-off-warning-first
- reviews: story:scrape-token-floor
- reviews: story:probes-before-head-checks
- reviews: story:bare-lf-head-malformed
revision: 1
---
Security review (aep:security-reviewer) of wave 2026-10-30-w01 at 458bca33: the cli unit
(config refusals, usage errors, ingress-off warning, scrape-token floor) and the server unit
(probes before head checks, bare-LF heads). Verdict CONFIRMED: nothing the wave introduced is
faulted; two pre-existing findings.

Invariants held, each pinned by a new case: no deployment-document refusal quotes a value (30
documents); no usage error echoes an argument (22 command lines); the scrape-token floor counts
the trimmed token; a target resembling a probe or public route still refuses a repeated
credential; no head shape lets a second request ride along.

Disposition: finding 1 fixed on impl/w01-sec-fixes (an unknown target name is never quoted).
Finding 2 kept: naming the unreadable path is how an operator finds a wrong path.

```findings
- file: crates/llm-gateway-cli/src/credential.rs
  line: 117
  category: boundary
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: named() quotes any --model/--route value of 64 or fewer [A-Za-z0-9._-] characters in credential:unknown-target, so a token-shaped key given as the alias reaches stderr
- file: crates/llm-gateway-cli/src/trusted.rs
  line: 106
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: every trusted-file refusal starts with the path verbatim, so a key passed as --config=<key> is printed in config:unreadable
```
