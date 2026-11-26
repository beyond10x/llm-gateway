---
format: aep.planning-md/3
id: review-result:adversary-w05-test-budget-pass-1
kind: review-result
status: active
title: 'Adversary pass 1 on the test budget and port race unit: needs change'
relations:
- reviews: story:cli-test-port-race
- reviews: story:test-step-within-budget
revision: 1
---
## Verdict

NEEDS-CHANGE on unit U1 of wave 2026-11-26-w05 (story:cli-test-port-race,
story:test-step-within-budget), commits aaf3f512..d19e6202, reviewed at the wave merge 4c81301e.
Red cases on adv/2026-11-26-w05 at 8e3d3a96: 3 of 4 new cases fail on the unit's tree.

## Findings

1. `crates/llm-gateway-cli/tests/security_w07_hosted.rs:310`: `closed_address()` drops its
   listener and a test hands that port to the shipped binary as an endpoint. Acceptance 2 of
   story:cli-test-port-race names every such test of the crate. Pre-existing, inside the story's
   acceptance. Blocker.
2. `crates/llm-gateway-cli/src/hosted.rs:1777`, `crates/llm-runpod/tests/security_w06_probe_buffer.rs:180`,
   `crates/llm-runpod/tests/transport.rs:897`: a probe of a port whose listener the test dropped,
   expected unreachable; another test's listener can take the port. Pre-existing.
3. `README.md:232`: the table of the binary's own lines lacks `metrics listening on <address>`,
   so the `RUST_LOG=warn` sentence above it is false. Introduced.
4. `checks/xtask/src/main.rs:90`: the 120 s verdict reads nextest's JUnit run time only; the build
   and the wait for a slot are not timed. Introduced.
5. `.config/nextest.toml:1`: no `nextest-version` requirement, so only CI holds the pinned
   version. Introduced. Note.

## Held

No test disappeared (1339 test functions at the base, 1349 after, every base name present). A
test over the slow timeout is killed and fails, and a run over the global timeout is cancelled and
fails, on nextest 0.9.143. CI runs the local gate's command, and checks the archive's SHA-256
before it runs it. The port-0 rule admits no other port. The metrics line comes first and no
reader matches it as the serving line. The four failures under plain `cargo test` are test
interference within one process, not a product defect.
