---
format: aep.planning-md/3
id: review-result:security-w06-credential-buffers
kind: review-result
status: active
title: 'Security review: credential-buffers-zeroed (w06)'
relations:
- reviews: story:credential-buffers-zeroed
revision: 1
---
Security review of story:credential-buffers-zeroed (implementation at a98c451); cases committed as 2e84387 on
`review/credential-buffers-security`: `crates/llm-gateway/tests/security_w06_head_buffer.rs`,
`crates/llm-gateway-cli/tests/security_w06_buffers.rs`, `crates/llm-runpod/tests/security_w06_probe_buffer.rs`.

```
unit: U4 story:credential-buffers-zeroed, implementation a98c451, review commit 2e84387
verdict: NEEDS-CHANGE
cases: executed 384→392, red 2
origin: introduced 3 / pre-existing 2 / undecided 0
wrote-outside-worktree: 5 paths under the assigned sec1/ scratch
needs-coordinator: the Runpod readiness probe lies outside U4's file list
```

Method: each case makes a 40-byte credential at run time, lets the code read, hold and release it, then scans
every `rw` mapping through `/proc/self/mem` for its last 16 bytes (needle held XOR-masked). A control case per
file shows the scan finds a copy freed unwiped.

Red at head: `a_pipelined_requests_credential_is_nowhere_in_this_process_once_the_gateway_has_answered`
(the `to_vec` of bytes past the head at `server.rs:632` holds the next request's credential and is dropped
unwiped) and `the_readiness_probe_leaves_the_vllm_key_nowhere_in_this_process_once_the_transport_is_gone`
(`probe.rs:104` builds the request in a growing String and ends with `clear()`). Green at head and red at base
`f3c076f4`: the main head path and both key-file cases, so the unit's fix is measured, not only argued.

Cleared: the overwrite pattern cannot be elided and covers spare capacity; `read_token`, `owner_verifier`,
`metrics_bearer` scan clean; `sha256` padding never reallocates; stdin and prompt readers are sized first;
the hosted probe request is exact-size; the Bedrock token goes through llm's `Secret`; no `unsafe`; no new
dependency; no credential in any format, log or refusal text.

Routing (coordinator): findings 1 and 2 are the story's own class (a buffer holding a credential freed
unzeroed) and are fixed in U4. Finding 3 was already fixed by 46e8090. Findings 4 and 5 correct the record.

```findings
[
{"file":"crates/llm-runpod/src/transport/probe.rs","line":104,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"the readiness probe writes the vLLM key into a String grown by push_str and ends with request.clear(), so the binary frees the key unzeroed on every Runpod wake probe"},
{"file":"crates/llm-gateway/src/server.rs","line":632,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"the to_vec of the bytes past the head holds a pipelined request's credential and serve drops it unwiped on every path that is not a relay"},
{"file":"docs/verification/2026-10-30-credential-buffers-zeroed.md","line":75,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the Not changed by this story section lists sha256, owner_verifier and TokenFile::resolve as unchanged although a98c451 changed all three"},
{"file":"docs/verification/2026-10-30-credential-buffers-zeroed.md","line":93,"category":"contract-drift","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the record says the bytes past the head hold no head bytes, which is false for pipelined requests"},
{"file":"docs/verification/2026-10-30-credential-buffers-zeroed.md","line":88,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the residual list omits llm 0.19.0's HeaderValue copy (llm-providers auth.rs:95), the sha256 message schedule on the stack (auth.rs:189) and the Runpod probe request"}
]
```
