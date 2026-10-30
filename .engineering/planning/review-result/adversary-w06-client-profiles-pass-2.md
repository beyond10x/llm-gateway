---
format: aep.planning-md/3
id: review-result:adversary-w06-client-profiles-pass-2
kind: review-result
status: active
title: 'Adversary pass 2: client-profiles-translated (w06)'
relations:
- reviews: story:client-profiles-translated
revision: 1
---
Adversary pass 2 (final) on story:client-profiles-translated, attacking fix 45272e7; cases committed as
a4d7053 in `crates/llm-gateway-cli/tests/adversary_w06_admits.rs`.

```
unit: U3 story:client-profiles-translated, fix 45272e7, adversary commit a4d7053
verdict: NEEDS-CHANGE
cases: executed 60→64, red 2
origin: introduced 3 / pre-existing 0 / undecided 0
wrote-outside-worktree: 7 paths under the assigned adv2/ scratch
needs-coordinator: who decided pass 1's finding 1 (answered below)
```

Red: `adversary_w06_p2_the_loom_no_profile_answer_does_not_deny_the_chat_translation_it_serves` (the reworded
no-profile line tells Loom no model takes its chat requests "relayed or translated" while the binary translates
one to a messages-only model) and `adversary_w06_p2_a_model_blocks_comment_lines_hold_no_quote_a_pasting_shell_would_open`
(the apostrophe in "Claude Code's" opens a quote in interactive zsh and swallows the export; measured with zsh 5.9.2).
Green guards: the variable follows each block's route in one answer; a Bedrock block exports it.
Suite: cli adversary_w06_admits 7/9, translation 26; gateway public_listing 28, adversary_w06_profiles 1. EXIT=101.

Not broken: the variable appears exactly on translated-only blocks (same `relays(wire)` test as the request
path); quoting `'1'` and the whole answer is valid POSIX sh; the fixture move leaves no dangling reference and
reaches the image build context; scenario, suite, status detail and CHANGELOG match the binary.

Answer to needs-coordinator: the pass 1 finding 1 decision (export `CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS`
for a translated-only model, and assert the structured-output request is still refused) was the wave
coordinator's, recorded in review-result:adversary-w06-client-profiles-pass-1. That Claude Code honours the
variable is cited from its published documentation, not observed; the docs must say so.
This was the last adversary pass; the coordinator verifies the correction.

```findings
[
{"file":"crates/llm-gateway/src/listing.rs","line":203,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the reworded no-profile line tells Loom no model takes its chat requests relayed or translated, while the binary translates a Loom chat request with a tool to a messages-only model in the same deployment (adversary_w06_admits.rs:616)"},
{"file":"crates/llm-gateway/src/listing.rs","line":265,"category":"property","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the new comment line's apostrophe opens a quote when the block is pasted into interactive zsh, swallowing export CLAUDE_CODE_DISABLE_STRUCTURED_OUTPUTS='1' (adversary_w06_admits.rs:646)"},
{"file":"crates/llm-gateway-cli/tests/adversary_w06_admits.rs","line":376,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"pass 1's acceptance assertion was turned around on a coordinator decision; the substitute rests on Claude Code honouring a variable cited from published documentation and never observed"}
]
```
