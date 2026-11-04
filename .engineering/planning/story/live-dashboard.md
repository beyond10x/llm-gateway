---
format: aep.planning-md/3
id: story:live-dashboard
kind: story
status: draft
title: A live dashboard and Prometheus series show every call, session and owned pod
relations:
- decomposes: epic:gateway-features
- serves: vision:portable-model-inference
- informed_by: story:usage-records
- informed_by: story:token-and-cache-metrics
scope:
- confidence: cited
  path: crates/llm-gateway/src/dashboard.rs
- confidence: cited
  path: crates/llm-gateway/src/usage.rs
- confidence: cited
  path: crates/llm-gateway/src/metrics.rs
- confidence: cited
  path: crates/llm-gateway/src/server.rs
- confidence: cited
  path: dashboard/src/App.vue
revision: 1
---
## Outcome

Every call yields a record with route, upstream, wire, status, queue wait, time to first token,
duration, tokens in and out, cache read and creation (decoded from the answer on all three wires),
client retries (`x-stainless-retry-count`, a repeated body digest), client disconnects and an
upstream 429 or 5xx with its `retry-after`. An in-flight registry, per-session token rates
(input tokens over time to first token, output tokens over streaming time; the session from
Claude Code's `x-claude-code-session-id`, Codex's `session-id`, else a hash of user agent and
client address) and a Runpod panel (balance, spend per hour, owned pods with state, GPU, price,
uptime, readiness, last call, watchdog heartbeat age) are served at `GET /dashboard` (one page,
Vue 3 and TypeScript built with Vite and pnpm into one inlined HTML file), `GET /dashboard/events`
(server-sent events) and `GET /dashboard/snapshot` (JSON), and as series on `GET /metrics`.

Session headers verified 2026-11-04 from captured requests: Claude Code 2.1.318 sends
`X-Claude-Code-Session-Id` (its `metadata.user_id` is a JSON text, not read); Codex 0.160.0 sends
`session-id` (hyphen).

## Acceptance

1. Fixture answers on the three wires, streamed and not, fill the token fields; a reported zero
   is zero, an unreported counter is absent.
2. `llmgw_tokens_total{model,direction}` and `llmgw_cache_tokens_total{model,kind}` sum them;
   time-to-first-token and queue-wait histograms, an in-flight gauge, retry counters, upstream
   status counters, Runpod balance, spend, owned pods and heartbeat age are series too.
3. The snapshot's TypeScript types are generated from Rust and drift-checked by `task docs`.
4. No session id, user id or credential appears whole in the snapshot or a log line.
