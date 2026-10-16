---
format: aep.planning-md/3
id: review-result:adversary-w11-routes-document-pass-1
kind: review-result
status: active
title: Adversary pass 1, story:routes-document (wave 2026-10-16-w02)
relations:
- reviews: story:routes-document
revision: 1
---
unit: story:routes-document, findings cover adv/routes-document at 3f78ef1 (unit head 15dae4fe)
verdict: NEEDS-CHANGE
cases: executed 230→233, red 3
origin: introduced 4 / pre-existing 1 / undecided 0
wrote-outside-worktree: 4 paths: 3 kept under ~/.cache/llm-gateway-m2b/adv-routes/, 1 temporary message file already deleted
needs-coordinator: whether a Messages route should send `anthropic-version` (the spec says "no static header")

**Diff (15dae4fe..3f78ef1):** `crates/llm-gateway-cli/tests/adversary_w02_routes.rs | 313 +` (a test file only), committed by `b10x-bot[bot]`.

**Red cases.** I ran each one alone before the suite.
1. `adversary_routes_a_messages_route_relayed_on_its_own_wire_is_sent_anthropic_version`. It first checks that `GET /` with UA `claude-cli` hands Claude Code `ANTHROPIC_MODEL='opus'`, and that check passed. Then it fails: `the relayed turn carries no anthropic-version: "POST /anthropic/v1/messages HTTP/1.1\r\nhost: …\r\nx-api-key: <credential>\r\ncontent-type: application/json\r\ncontent-length: 101\r\nconnection: close"`, left `None`, right `Some("2023-06-01")`.
2. `adversary_routes_the_output_clamps_count_to_a_messages_route_is_sent_anthropic_version`. Same failure on `POST /anthropic/v1/messages/count_tokens`.
3. `adversary_routes_a_route_name_of_llms_full_length_is_served`. The start is refused: `refused config:value: routes.rrrr…(256) cannot be served: a label must be 1..=256 bytes`.

**Suite:** `RUSTUP_TOOLCHAIN=1.98.0 cargo test -p b10x-llm-gateway-cli --locked --no-fail-fast` exited 101. 230 passed and 3 failed (my three). The 230 is that same run without my file, not a separate run. fmt and clippy are clean on my file.

| # | file:line | Finding | Verdict | Origin | Reached by |
|---|---|---|---|---|---|
| 1 | crates/llm-gateway-cli/src/routes.rs:358 | The byte relay on a Messages route sends no `anthropic-version` | NEEDS-CHANGE | introduced | `GET /` hands Claude Code exactly this route. Every Claude Code request has `max_tokens`. The docs' endpoint is `https://api.anthropic.com/v1`. Possible fix: send `llm_messages::ANTHROPIC_VERSION` on a `messages` route endpoint |
| 2 | crates/llm-gateway/src/relay.rs:1425 | The output clamp's `count_tokens` call to that endpoint has the same omission, so the count fails without a word and the clamp never clamps | NEEDS-CHANGE | introduced | Same path. A plain route's default window (200000) always turns the clamp on |
| 3 | crates/llm-gateway-cli/src/routes.rs:379 | A route name llm accepts (more than about 247 bytes) is refused at start, because the target id `<route>.<wire>` goes over Label's 256-byte limit | INFEASIBLE | introduced | Nothing found. I built this case myself |
| 4 | website/docs/guides/translate-between-wires.md:153 | The pasted `config_digest` `767d4830…` is not the SHA-256 of the `gateway.toml` shown (`14bffd40…`). `binding_revision` does match the `routes.yaml` shown | CONFIRMED | introduced | Docs readers; the page says "every output pasted from a run" |
| 5 | crates/llm-gateway/src/listing.rs:124 | `GET /` profiles count only a model's own upstream wire. On the docs example, Codex gets no profile and Claude Code gets `codex` (the Anthropic route), the opposite of the page's pairing table | CONFIRMED | pre-existing | `profiled()` is the same at 20f48f2c; routes make it visible |

**Not broken, one line each:**
- The gateway's `serde_yaml` read of `auth.secret` matches llm on all 22 probe documents. These covered anchors, aliases, merge keys, `!!str`/custom tags, quoted, integer, `~` and explicit keys, repeated keys and block scalars. llm refuses merge keys and repeats. The probe file is deleted.
- `base_url` cases: userinfo and query are refused; IPv6 `[::1]` and hex `0x7f.1` are loopback; `[::ffff:127.0.0.1]` with a credential is refused; dot segments, default port and uppercase scheme are normalised by llm before the gateway checks.
- Secrets: `env:` and `keychain:` are refused; `file:` paths with `..`, relative paths, a directory, a FIFO, a symlink and group- or world-writable files are all refused.
- Alias injection: metrics, `/v1/models`, profiles (TOML, shell and HTML quoting) and logs (Debug-quoted) escape every printable-ASCII route name.
- A direct `<endpoint>/<model-id>` gets 404.
- The docs' `binding_revision` matches the SHA-256 of the docs' `routes.yaml`.

**Paths written outside the worktree:**
- `~/.cache/llm-gateway-m2b/adv-routes/suite.log`
- `~/.cache/llm-gateway-m2b/adv-routes/doc-routes.yaml`
- `~/.cache/llm-gateway-m2b/adv-routes/doc-gateway.toml`
- `~/.cache/claude-tmp/tmp.YTjiK1lyfZ` (deleted)

Lease `adv-routes-document-p1` is released. Nothing pushed.

```findings
[{"file":"crates/llm-gateway-cli/src/routes.rs","line":358,"category":"acceptance","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a same-wire Messages request on a route, the route GET / hands Claude Code, reaches the endpoint without anthropic-version, which Anthropic's API requires"},
 {"file":"crates/llm-gateway/src/relay.rs","line":1425,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the output clamp's count_tokens to a route's Messages endpoint carries no anthropic-version, so on Anthropic the count fails and the clamp never applies"},
 {"file":"crates/llm-gateway-cli/src/routes.rs","line":379,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"a route name llm admits above about 247 bytes refuses the start because the target id <route>.<wire> exceeds the 256-byte label bound"},
 {"file":"website/docs/guides/translate-between-wires.md","line":153,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the pasted config_digest is not the SHA-256 of the gateway.toml the page shows, so that output came from a different document"},
 {"file":"crates/llm-gateway/src/listing.rs","line":124,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"GET / profiles count only declared wires, so on the routes example Codex gets none and Claude Code gets the Anthropic route, the opposite of the page's pairing table"}]
```
