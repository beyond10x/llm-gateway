---
format: aep.planning-md/3
id: review-result:adversary-w11-routes-document-pass-2
kind: review-result
status: active
title: Adversary pass 2, story:routes-document (wave 2026-10-16-w02)
relations:
- reviews: story:routes-document
revision: 1
---
unit: story:routes-document, correction 3f78ef1...deca5af, findings cover adv/routes-document at c2831db
verdict: INFEASIBLE
cases: executed 237→244, red 1
origin: introduced 1 / pre-existing 0 / undecided 0
wrote-outside-worktree: 4 paths, under ~/.cache/llm-gateway-m2b/adv-routes-p2/
needs-coordinator: whether Anthropic accepts the clamp's count. The relay sends `count_tokens` the whole request (`max_tokens`, `stream`), and a client's `anthropic-beta` is never forwarded. I don't know if Anthropic refuses either: it needs a live call, which I did not make.

The five decisions are implemented as stated. One small contract gap is the only red case.

**1. Diff (deca5af..HEAD):** `crates/llm-gateway-cli/tests/adversary_w02_routes_p2.rs | 442 +`. It is a test file only, committed as c2831db by `b10x-bot[bot]`. Nothing pushed.

**2. Cases** (`cargo test --test adversary_w02_routes_p2`, run alone first: 6 passed, 1 failed):

| Case | Asserts | Now |
|---|---|---|
| `…the_reference_page_publishes_the_anthropic_version_header_refusal` | the page's `auth.header` rule names the new refusal | red: `the auth.header rule does not name the refusal the code gives: \| An \`api-key\` endpoint's \`auth.header\` is one a hosted model's \`credential_header\` may be. \| …` |
| `…a_clients_own_anthropic_headers_never_reach_a_messages_route` | turn and count each carry exactly one `anthropic-version` (llm's) and the endpoint's own `x-api-key`; no `anthropic-beta` | green |
| `…a_translated_turn_to_a_messages_route_carries_one_anthropic_version` | chat to messages: exactly one header, not two | green |
| `…a_non_messages_route_is_sent_no_anthropic_version` | chat and responses routes on their own wire carry none | green |
| `…an_api_key_header_named_anthropic_version_is_refused_at_start` | `Anthropic-Version` as `auth.header` is refused `config:value routes.lab.auth.header` | green |
| `…a_route_at_the_bound_on_the_longest_wire_is_served_and_listed` | a 246-byte route on responses (a 256-byte target id) is relayed 200 and listed in `/v1/routes` | green |
| `…a_multi_byte_route_name_never_reaches_the_gateways_bound` | llm refuses non-ASCII names, and the refusal does not echo the name | green |

**3. Suite:** `RUSTUP_TOOLCHAIN=1.98.0 cargo test -p b10x-llm-gateway-cli --locked --no-fail-fast` exited 101: 243 passed, 1 failed (mine). The 237 is that same run with my file's 7 left out. fmt and clippy are clean on the file.

**4. Findings**

| # | file:line | Finding | Verdict | Origin | Reached by |
|---|---|---|---|---|---|
| 1 | website/docs/reference/deployment-document.md:191 | The start now refuses `auth.header: anthropic-version` on a `messages` endpoint (`crates/llm-gateway-cli/src/routes.rs:397`). The page's rule admits any header a hosted `credential_header` may be, and the spec's rule (`spec/domains/deployment.yaml:504`) has the same gap. Fix: add the case to that row and to the spec | INFEASIBLE | introduced | nothing found; I built this case myself |

**5. Attacked and not broken**
- The pass-1 assertions are intact. Cases 1 and 2 are unchanged and now green. Case 3 was re-pinned as decided and is now stricter: it checks the bound, bound+1 and 256, and that no long name is echoed.
- Bound mutants are caught. A constant of 245 fails the page/spec check; 247 fails the start at the bound; dropping `route_name` or quoting the whole name fails the existing tests.
- The guide re-ran from its own files: the `/v1/models`, `/v1/routes` and both `GET /` pastes match byte for byte. `config_digest` 767d… is right, so pass 1's finding 4 was wrong. `binding_revision` is 4444…
- I did not re-run the two `translation-unsupported` curls: their targets are api.anthropic.com and openrouter.ai, not loopback.

**6. Paths outside the worktree**
- `~/.cache/llm-gateway-m2b/adv-routes-p2/extract.py`, `probe.sh` (the guide probe)
- `~/.cache/llm-gateway-m2b/adv-routes-p2/p2-alone.log`, `suite.log`
- `~/.cache/llm-gateway-m2b/adv-routes-p2/doc/` (probe output with fake keys), already deleted
- `~/.cache/claude-tmp/adv-p2-msg.*` (commit message), already deleted

Lease `adv-routes-document-p2` is released.

```findings
[{"file":"website/docs/reference/deployment-document.md","line":191,"category":"contract-drift","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"the start refuses auth.header anthropic-version on a messages endpoint, but the page's auth.header rule (and the spec's) still admits any header a hosted credential_header may be"}]
```
