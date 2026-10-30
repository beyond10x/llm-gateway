---
format: aep.planning-md/3
id: review-result:adversary-w06-client-profiles-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: client-profiles-translated (w06)'
relations:
- reviews: story:client-profiles-translated
revision: 1
---
Adversary pass 1 on story:client-profiles-translated (impl/client-profiles-translated at 4b2b03d; cases
committed as 053e473 in `crates/llm-gateway-cli/tests/adversary_w06_admits.rs` and
`crates/llm-gateway/tests/adversary_w06_profiles.rs`).

```
unit: U3 story:client-profiles-translated, branch impl/client-profiles-translated at 4b2b03d (adversary commit 053e473 on top)
verdict: NEEDS-CHANGE
cases: executed 126→132, red 3
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 1 (commit message file)
needs-coordinator: whether the acceptance covers every Claude Code request or only its first turn (finding 1)
```

Red: `adversary_w06_claude_codes_structured_output_request_reaches_the_translated_model_get_root_names`
(GET / profiles Claude Code for `chat-up`; the recorded `3-structured-output.request.txt` is refused 400
`translation-unsupported`), `adversary_w06_a_bedrock_model_claude_code_is_not_admitted_to_is_not_said_to_lack_the_messages_wire`,
`adversary_w06_the_no_profile_answer_does_not_deny_a_wire_a_model_declares`. Green guards: Claude Code's second
turn on a translated chat route is carried; the translation guide's pasted GET / answers match the binary; the
Bedrock guide's model is still profiled for Claude Code and Codex.

Suite: gateway lib 47, public_listing 26, dependency_boundary 3, gateway 24 ok, adversary_w06_profiles 0/1;
cli translation 26 ok, adversary_w06_admits 3 passed 2 failed. EXIT=101.

Not broken: logging silenced only on the composing thread during prepare; upstream wire choice; models with
native and translated wires; Loom chat profile; no translator; /v1/models vs GET / split; dependency boundary;
the changed scenario asserts the new rule (a behaviour change, not a relaxed assertion); user-agent matching;
composition cost. Judgement only: a translator whose `admits` panics stops the gateway from starting.

```findings
[
  {"file": "crates/llm-gateway-cli/src/translate.rs", "line": 633, "category": "acceptance", "severity": "blocker", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "GET / profiles Claude Code for a translated model from its first turn alone, and Claude Code's recorded structured-output request to that alias is refused translation-unsupported (adversary_w06_admits.rs:373), contradicting the acceptance and docs/gateway.md:188-189"},
  {"file": "crates/llm-gateway/src/listing.rs", "line": 194, "category": "contract-drift", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "the no-profile text says no model declares the client's wire with tool calling parsed when a translated-only model declaring it was left out, reached through the binary by a Bedrock model with max_output_tokens under 32000 (adversary_w06_admits.rs:505, adversary_w06_profiles.rs:86)"},
  {"file": "crates/llm-gateway-cli/src/translate.rs", "line": 576, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "the shipped binary's GET / answer now depends on an authored test fixture under another crate's tests/ directory, so editing that fixture changes production profiles"}
]
```
