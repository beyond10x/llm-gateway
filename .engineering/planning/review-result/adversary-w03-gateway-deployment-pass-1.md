---
format: aep.planning-md/3
id: review-result:adversary-w03-gateway-deployment-pass-1
kind: review-result
status: active
title: Wave 2026-10-08-w03 adversary, story:gateway-deployment, pass 1
relations:
- reviews: story:gateway-deployment
revision: 1
---
## Pass

Wave 2026-10-08-w03 adversary, story:gateway-deployment, pass 1, against `impl/gateway-deployment`
at `bedc34e` (base `524336b`). The adversary committed `f28f34f` (`crates/llm-gateway-cli/tests/adversary_w03.rs`,
three cases), red: `test result: FAILED. 0 passed; 3 failed`.

| Case | Red output |
|---|---|
| `adv_d2_a_request_waiting_for_its_pod_cannot_hold_the_stop_past_twice_the_read_timeout` | `the stop took 8.502534332s, more than twice the 1 s read_timeout` |
| `adv_k27_an_idle_timeout_of_zero_never_stops_the_pod_a_waiting_request_started` | `the cleanup pass stopped the pod the waiting request started, before it served it` |
| `adv_w7_after_a_failed_connection_the_next_request_reaches_a_replacement` | `left: "pod1-8000.proxy.runpod.net" right: "pod1-8000.proxy.runpod.net"`, 1 pod created |

Attacked without a break: client headers and the owner secret never reach the pod (all three
wires); CR, LF, space and non-printable key bytes are refused by `TargetBearer::new`; `Debug` of
`TargetBearer`, `VllmKey`, `VllmKeys` is redacted; `crates/llm-gateway/Cargo.toml` declares no
dependency; no `main.rs` path or feature reaches `start_relaying`; the two new scenarios go red on
a mutant that drops the bearer or forwards the client header (read, not run).

Weak test noted outside the findings: `crates/llm-gateway-cli/tests/relaying.rs:283` greps
`main.rs` for the emulator only.

```findings
[
  {"file": "crates/llm-gateway-cli/src/relaying.rs", "line": 318, "category": "contract-drift", "severity": "warning", "verdict": "INFEASIBLE", "origin": "introduced", "message": "a request waiting in acquire holds the graceful stop for up to start_wait_seconds, breaking matrix row D2's bound of twice read_timeout (8.5 s measured against 2 s); only start_relaying reaches it, and the shipped binary does not call that"},
  {"file": "crates/llm-gateway-cli/src/relaying.rs", "line": 130, "category": "boundary", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "idle_timeout_minutes = 0 lets the cleanup pass that first observes the pod ready stop it before the request that started it is served, against the unit's own DECIDED note in deployment.yaml"},
  {"file": "crates/llm-gateway-cli/src/relaying.rs", "line": 329, "category": "contract-drift", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "invalidate does nothing, so the pod that failed is handed out again, against covered matrix row W7 and docs/gateway.md:162"},
  {"file": "crates/llm-gateway-cli/src/relaying.rs", "line": 402, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "introduced", "message": "shutdown stops no pod, so pods created during the run, including during the drain, keep billing until a later start sweeps them; found by reading, not tested, and only start_relaying reaches it"},
  {"file": "docs/hosting.md", "line": 310, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "the table says no vLLM key value passes through this process, but the process reads it and now sends it in every relayed request"}
]
```
