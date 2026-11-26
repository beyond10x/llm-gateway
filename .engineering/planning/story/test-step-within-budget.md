---
format: aep.planning-md/3
id: story:test-step-within-budget
kind: story
status: active
title: The test step of the gate ends within its budget and kills a test at 60 s
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T02:02:41Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T02:02:41Z", actor: "human:operator", revision: 3}
---
## Outcome

The test step of the gate ends within 120 s of wall time, a test over 60 s is killed and counts as
failed, and the step prints its ten slowest tests.

## Why

Measured on the build node, 2026-11-25, `cargo test --workspace --locked --no-fail-fast`, Rust
1.98.0:

- on `main` at the ESS pin change, with a cold build: 233 s (88 s build, 105 s of tests summed
  over the binaries, 37 s fetching the artifacts);
- on `wave/2026-11-25-w01` at 2a2a2567, with a warm build: 139 s (12 s build, 120 s of tests
  summed).

The step runs 128 test executables one after another (120 of them `tests/*.rs` files, 8 unit-test
binaries) and five doc-test runs. The eight slowest binaries of the second run took 12.0, 10.3,
10.0, 9.2, 7.3, 6.4, 5.2 and 5.1 s. Nothing kills a test that hangs, and the step does not fail
when it runs over its budget.

## Acceptance

1. One integration-test binary per crate: `tests/main.rs` with the present files as modules. The
   count of test executables before and after is in the pull request.
2. The runner kills a test at 60 s and counts it as failed, and prints the ten slowest tests.
3. The step fails when it runs over 120 s, in the local gate and in CI.
4. The step's wall time before and after, measured on the same machine with a warm build, is in
   the pull request, and the second number is under 120 s.
5. No test is removed or loosened to reach the time. A test that waits on a clock is moved to a
   clock the test owns.
