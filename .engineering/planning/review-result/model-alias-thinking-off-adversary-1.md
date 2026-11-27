---
format: aep.planning-md/3
id: review-result:model-alias-thinking-off-adversary-1
kind: review-result
status: active
title: Adversary pass 1 on the model alias with thinking off (2026-11-27)
relations:
- reviews: story:model-alias-thinking-off
revision: 1
---
## Adversary pass 1 on `[aliases.<alias>]` (2026-11-27)

Target: branch `feat/qwen-plain-effort-none` at 46ac20db6. Red tests: commit bfcafbe38 (taken
into the branch as df58bdab5). Suite with the cases: 701 run, 699 passed, 2 failed.

Findings:

- F1, blocker, introduced (`crates/llm-gateway-cli/src/watchdog.rs`): the heartbeat records an
  alias's calls under the alias name and the watchdog read only the model's name, so a pod busy
  with an alias call was terminated as idle once the model's own name went quiet.
  Test `watchdog::tests::adversary_alias_a_pod_busy_under_its_alias_is_not_terminated_as_idle`,
  red `left: [("pod1", Idle)] right: []`. Fixed in 4d2e23e73: the watchdog adds the activity of
  every alias to its model's; the pod is idle only when every name is idle.
- F2, warning, introduced (`crates/llm-gateway/src/relay.rs`): an alias of a Runpod model whose
  `wires` lack `messages` had its messages requests translated, and the translator drops the
  forced effort, so the pod got `"reasoning_effort":"high"`. Fixed in 4d2e23e73: such an alias is
  refused at start (`config:value`, naming `aliases.<alias>.model` and the model); test
  `config::adversary_alias_of_a_model_without_the_messages_wire_is_refused_at_start`.
- F3, note, introduced (`crates/llm-gateway-cli/src/serve.rs`): `GET /v1/routes/<alias>` answered
  `route-unknown` while the alias was served. Fixed in 4d2e23e73: the alias is listed as a route
  with its model's targets.

Attacked without a finding: duplicate, null and non-object `chat_template_kwargs` and
`output_config`; escaped key names; nested look-alike keys; overlapping byte edits; the forced
`stream:true` rewrite; the model's own requests byte for byte; pool acquire, invalidate, wake and
status under the alias; one pod start; name clashes with hosted, Bedrock and route names.

After the fixes: `task check` exit 0, 1512 tests run, 1512 passed, 1 skipped; conformance 435 of
435 on three runs.
