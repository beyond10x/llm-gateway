---
format: aep.planning-md/3
id: review-result:adversary-w12-model-profile-fallback-pass-1
kind: review-result
status: active
title: Wave 2026-10-16-w03 adversary, story:model-profile-fallback, pass 1
relations:
- reviews: story:model-profile-fallback
revision: 1
---
unit: story:model-profile-fallback, adversary pass 1, covering adv/model-profile-fallback at 85c4555 plus dbaaf62 (tests only)
verdict: red (two confirmed findings and one infeasible, all introduced)
cases: executed 403→405, red 2
origin: n/a (adversary: introduced 3, pre-existing 0, undecided 0)

1. Red: `crates/llm-gateway-cli/tests/adversary_w12_profiles.rs:184`: the output clamp and `GET /v1/models` use the first profile's `max_model_len` whichever profile served, so a call served by a shorter fallback is clamped against a window its pod does not have (the fallback example in `docs/model-profiles.md` reaches it).
2. Red: `crates/llm-runpod/tests/adversary_w12_profiles.rs:137`: the profile loop reads no clock and sends creates on the next profile after the start's deadline has passed; the same gap existed within one profile's GPU list.
3. Infeasible offline: no connectors answer for a Runpod `pod.create` carries the failure code `capacity` (a 4xx is `refused`, a 5xx `unknown`, which ends as `Lost`), so the production transport never yields `NoCapacity`; the fallback is reached against the emulator and the fixture only, which is what acceptance 2 asks.
4. Attacked and not broken: lost answer, other refusals and a spend refusal end the start; the spend check runs before each create; adopted pods report their profile from `gpu.id`; a one-profile model keeps its errors and keys; the per-step `results` correction of two scenarios matches the runner and the sibling scenarios.
5. Decisions (coordinator): the clamp takes its window from the serving lease's profile, and the model listing states the smallest window among the model's profiles; the profile loop and the GPU loop stop once the start's deadline has passed; finding 3 is asked of connectors (mark Runpod's stock-out create answer `capacity`), and `docs/model-profiles.md` states that the fallback needs it.

```findings
[{"file":"crates/llm-gateway-cli/src/relaying.rs","line":962,"category":"acceptance","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"the output clamp and model listing use the first profile's max_model_len, so a call served by a shorter fallback profile is clamped against a window its pod does not have"},
 {"file":"crates/llm-runpod/src/provider.rs","line":576,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"the profile loop reads no clock and sends creates on the next profile after the start's startup deadline has passed"},
 {"file":"crates/llm-runpod/src/transport/connectors.rs","line":782,"category":"contract-drift","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"no connectors producer emits failure code capacity for a Runpod pod.create, so the production transport never yields NoCapacity and the fallback is reached only against the emulator and fixture"}]
```
