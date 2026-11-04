import type { CallRecord, Snapshot } from './generated/snapshot.ts';

const ROUTES = ['qwen-coder', 'claude-sonnet', 'claude-haiku'];
const SESSIONS = [
  { session: '3f9a1c0e', label: 'claude-code' },
  { session: '8b27d4aa', label: 'codex' },
  { session: 'c01e5f72', label: 'curl/8.11' },
];

export function fixture(now: number): Snapshot {
  const calls: CallRecord[] = [];
  for (let i = 0; i < 40; i++) {
    const route = ROUTES[i % 3]!;
    const s = SESSIONS[i % 3]!;
    const failed = i % 11 === 4;
    const refused = i % 17 === 9;
    calls.push({
      id: 1000 - i,
      started_ms: now - i * 37_000 - 2_000,
      route: refused ? null : route,
      upstream: refused ? null : route.startsWith('claude') ? 'anthropic' : 'runpod',
      wire: route.startsWith('claude') ? 'messages' : i % 2 ? 'responses' : 'chat',
      status: refused ? 401 : failed ? 429 : 200,
      target_status: refused ? null : failed ? 429 : 200,
      outcome: refused ? 'refused' : failed ? 'upstream_error' : 'ok',
      refusal: refused ? 'unknown model' : null,
      queue_wait_ms: route === 'qwen-coder' ? 12 + (i % 5) * 40 : null,
      ttft_ms: refused || failed ? null : 180 + ((i * 97) % 1400),
      duration_ms: refused ? 1 : 900 + ((i * 1311) % 24_000),
      response_bytes: 4000 + i * 311,
      input_tokens: refused || failed ? null : 2_000 + ((i * 7919) % 90_000),
      output_tokens: refused || failed ? null : 40 + ((i * 131) % 3_000),
      cache_read_tokens: route.startsWith('claude') && !failed ? 60_000 + i * 900 : null,
      cache_creation_tokens: route.startsWith('claude') && !failed ? (i % 4 === 0 ? 2_400 : 0) : null,
      client_retry: i % 13 === 3 ? 2 : i % 5 === 0 ? 0 : null,
      repeated_body: i % 13 === 3,
      client_disconnected: i === 7,
      upstream_retry_after: failed ? '30' : null,
      session: s.session,
      user_agent: s.label,
    });
  }
  return {
    format: 'llm-gateway.dashboard/1',
    now_ms: now,
    started_ms: now - 5 * 3600_000 - 17 * 60_000,
    calls,
    in_flight: [
      { id: 1001, started_ms: now - 2_400, route: 'qwen-coder', wire: 'chat', state: 'streaming', elapsed_ms: 2_400, session: '8b27d4aa' },
      { id: 1002, started_ms: now - 41_000, route: 'claude-sonnet', wire: 'messages', state: 'waiting-upstream', elapsed_ms: 41_000, session: '3f9a1c0e' },
    ],
    sessions: [
      { session: '3f9a1c0e', label: 'claude-code', calls: 214, input_tokens: 1_840_221, output_tokens: 98_412, cache_read_tokens: 14_220_900, input_tps: 6120.4, output_tps: 71.3, last_ms: now - 4_000 },
      { session: '8b27d4aa', label: 'codex', calls: 88, input_tokens: 2_920_004, output_tokens: 140_881, cache_read_tokens: 0, input_tps: 9412.8, output_tps: 118.6, last_ms: now - 2_400 },
      { session: 'c01e5f72', label: 'curl/8.11', calls: 3, input_tokens: 120, output_tokens: 0, cache_read_tokens: 0, input_tps: null, output_tps: null, last_ms: now - 3 * 3600_000 },
    ],
    routes: [
      { route: 'qwen-coder', calls: 131, errors: 4, input_tokens: 3_010_442, output_tokens: 161_002, cache_read_tokens: 0, cache_creation_tokens: 0, in_flight: 1 },
      { route: 'claude-sonnet', calls: 160, errors: 2, input_tokens: 1_500_120, output_tokens: 70_211, cache_read_tokens: 12_440_000, cache_creation_tokens: 380_000, in_flight: 1 },
      { route: 'claude-haiku', calls: 14, errors: 0, input_tokens: 250_000, output_tokens: 8_080, cache_read_tokens: 1_780_900, cache_creation_tokens: 22_000, in_flight: 0 },
    ],
    provider: {
      name: 'runpod',
      read_ms: now - 21_000,
      balance_usd: 87.412,
      spend_per_hour_usd: 2.39,
      other_pods: 1,
      pods: [
        { id: 'abc123xyz', name: 'b10x-llm-qwen-coder', model: 'Qwen/Qwen3-Coder-30B-A3B-Instruct', state: 'RUNNING', gpu: 'NVIDIA H100 80GB HBM3', cost_per_hour_usd: 2.39, uptime_s: 3 * 3600 + 1240, vllm_ready: true, last_call_ms: now - 2_400, in_flight: 1 },
        { id: 'def456uvw', name: 'b10x-llm-small', model: null, state: 'STARTING', gpu: 'NVIDIA L40S', cost_per_hour_usd: 0.86, uptime_s: 95, vllm_ready: false, last_call_ms: null, in_flight: 0 },
      ],
      watchdog_heartbeat_age_ms: 420_000,
    },
  };
}
