---
format: aep.planning-md/3
id: review-result:adversary-w13-credential-set-command-pass-1
kind: review-result
status: active
title: Wave 2026-10-17-w01 adversary, story:credential-set-command, pass 1
relations:
- reviews: story:credential-set-command
revision: 1
---
## Verdict

needs-change. Pass 1 on `impl/credential-set-command` at cfa2281; failing tests in
`crates/llm-gateway-cli/tests/adversary_w13_credential.rs` (f395739), 2 red.

## Findings

```findings
- {file: crates/llm-gateway-cli/src/credential.rs, line: 551, category: acceptance, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "clap's usage error repeats a key passed as an argument on stderr"}
- {file: crates/llm-gateway-cli/src/hosted.rs, line: 322, category: boundary, severity: note, verdict: NEEDS-CHANGE, origin: introduced, message: "an interim 1xx answer is taken as final, so a key the endpoint rejects with 401 is stored"}
- {file: crates/llm-gateway-cli/src/hosted.rs, line: 295, category: contract-drift, severity: warning, verdict: CONFIRMED, origin: introduced, message: "with no base path the probe asks /models while the gateway sends under /v1"}
- {file: crates/llm-gateway-cli/src/hosted.rs, line: 322, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "the probe has no total time limit; a server sending one byte per read holds it for hours"}
- {file: crates/llm-gateway-cli/src/credential.rs, line: 166, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "the --model value is quoted whole in the refusal, a terminal hangup accepts a partial key, and an outside signal leaves echo off"}
```

## Held

Input rules, probe path with a base path, credential and static headers, 401/403 and transport
failure write nothing, `--route` resolution, the directory and temp-file rules, the read-back
through the server's reader, plain http to a remote host refused, no redirect followed, target
loading reads no credential, and the serving command line unchanged.

## Disposition

All five go to one fix unit on `adv/credential-set-command`; an outside SIGTERM during the prompt
leaving echo off is accepted as a known limit.
