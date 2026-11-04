import type { AttemptRecord, CallRecord, Snapshot } from './extra.ts';

const ROUTES = ['qwen-coder', 'claude-sonnet', 'claude-haiku'];
const SESSIONS = [
  { session: '3f9a1c0e', label: 'claude-code' },
  { session: '8b27d4aa', label: 'codex' },
  { session: 'c01e5f72', label: 'curl/8.11' },
];

// Fixed points in time, so the page's countdowns visibly tick down.
const T0 = Date.now();
const RESET_5H_S = Math.floor((T0 + 2 * 3600_000 + 13 * 60_000) / 1000);
const RESET_7D_S = Math.floor((T0 + 3 * 86_400_000 + 5 * 3600_000) / 1000);

const RATE_LIMITED = '{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your rate limit."}}';
const OVERLOADED = '{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}';

function attempts(i: number, failed: boolean, route: string): AttemptRecord[] {
  if (failed) {
    return [
      { attempt: 1, status: 429, duration_ms: 310, wait_ms: 0, retry_after: '30', headers: {}, error_body: RATE_LIMITED },
      { attempt: 2, status: 429, duration_ms: 280, wait_ms: 30_000, retry_after: '30', headers: {}, error_body: RATE_LIMITED },
    ];
  }
  if (route.startsWith('claude') && i % 7 === 2) {
    return [
      { attempt: 1, status: 529, duration_ms: 1_204, wait_ms: 0, retry_after: null, headers: {}, error_body: OVERLOADED },
      { attempt: 2, status: 200, duration_ms: 8_812, wait_ms: 2_000, retry_after: null, headers: {}, error_body: null },
    ];
  }
  return [];
}

export function fixture(now: number): Snapshot {
  const calls: CallRecord[] = [];
  for (let i = 0; i < 40; i++) {
    const route = ROUTES[i % 3]!;
    const s = SESSIONS[i % 3]!;
    const failed = i % 11 === 4;
    const refused = i % 17 === 9;
    const done = !refused && !failed;
    calls.push({
      id: 1000 - i,
      started_ms: now - i * 37_000 - 2_000,
      route: refused ? null : route,
      upstream: refused ? null : route.startsWith('claude') ? 'anthropic' : 'runpod',
      upstream_model: refused ? null : route === 'claude-sonnet' ? 'claude-sonnet-4-5' : route === 'claude-haiku' ? 'claude-haiku-4-5' : 'Qwen/Qwen3-Coder-30B-A3B-Instruct',
      wire: route.startsWith('claude') ? 'messages' : i % 2 ? 'responses' : 'chat',
      status: refused ? 401 : failed ? 429 : 200,
      target_status: refused ? null : failed ? 429 : 200,
      outcome: refused ? 'refused' : failed ? 'upstream_error' : 'ok',
      refusal: refused ? 'unknown model' : null,
      queue_wait_ms: route === 'qwen-coder' ? 12 + (i % 5) * 40 : null,
      ttft_ms: done ? 180 + ((i * 97) % 1400) : null,
      duration_ms: refused ? 1 : 900 + ((i * 1311) % 24_000),
      response_bytes: 4000 + i * 311,
      input_tokens: done && i % 19 !== 6 ? 2_000 + ((i * 7919) % 90_000) : null,
      output_tokens: done && i % 19 !== 6 ? 40 + ((i * 131) % 3_000) : null,
      cache_read_tokens: route.startsWith('claude') && !failed ? 60_000 + i * 900 : null,
      cache_creation_tokens: route.startsWith('claude') && !failed ? (i % 4 === 0 ? 2_400 : 0) : null,
      input_tps: done && i % 19 !== 6 ? 3_000 + ((i * 4111) % 9_000) : null,
      output_tps: done && i % 19 !== 6 ? 40 + ((i * 37) % 120) : null,
      usage_note: done && i % 19 === 6 ? 'undecodable(br)' : null,
      client_retry: i % 13 === 3 ? 2 : i % 5 === 0 ? 0 : null,
      repeated_body: i % 13 === 3,
      client_disconnected: i === 7,
      upstream_retry_after: failed ? '30' : null,
      attempts: attempts(i, failed, route),
      session: s.session,
      user_agent: s.label,
    });
  }
  const retryAt = now + 25_000 - ((now - T0) % 25_000);
  return {
    format: 'llm-gateway.dashboard/1',
    now_ms: now,
    started_ms: now - 5 * 3600_000 - 17 * 60_000,
    calls,
    in_flight: [
      { id: 1001, started_ms: now - 2_400, route: 'qwen-coder', wire: 'chat', state: 'streaming', attempt: 1, retry_at_ms: null, elapsed_ms: 2_400, session: '8b27d4aa' },
      { id: 1002, started_ms: now - 41_000, route: 'claude-sonnet', wire: 'messages', state: 'waiting-upstream', attempt: 1, retry_at_ms: null, elapsed_ms: 41_000, session: '3f9a1c0e' },
      { id: 1003, started_ms: now - 6_000, route: 'claude-haiku', wire: 'messages', state: 'waiting-retry', attempt: 2, retry_at_ms: retryAt, elapsed_ms: 6_000, session: '3f9a1c0e' },
    ],
    sessions: [
      { session: '3f9a1c0e', label: 'claude-code', calls: 214, input_tokens: 1_840_221, output_tokens: 98_412, cache_read_tokens: 14_220_900, input_tps: 6120.4, output_tps: 71.3, last_ms: now - 4_000 },
      { session: '8b27d4aa', label: 'codex', calls: 88, input_tokens: 2_920_004, output_tokens: 140_881, cache_read_tokens: 0, input_tps: 9412.8, output_tps: 118.6, last_ms: now - 2_400 },
      { session: 'c01e5f72', label: 'curl/8.11', calls: 3, input_tokens: 120, output_tokens: 0, cache_read_tokens: 0, input_tps: null, output_tps: null, last_ms: now - 3 * 3600_000 },
    ],
    routes: [
      { route: 'qwen-coder', calls: 131, errors: 4, input_tokens: 3_010_442, output_tokens: 161_002, cache_read_tokens: 0, cache_creation_tokens: 0, uncached_input_tokens: 3_010_442, input_tps: 9_212.5, output_tps: 112.4, in_flight: 1, upstream_model: 'Qwen/Qwen3-Coder-30B-A3B-Instruct', upstream: { source: 'vllm', read_ms: now - 6_000, prefix_cache_hit_ratio: 0.457, prefix_cache_hit_tokens: 1_376_200, prefix_cache_query_tokens: 3_011_400, requests_running: 1, requests_waiting: 0, kv_cache_usage: 0.31, ready: true } },
      { route: 'claude-sonnet', calls: 160, errors: 2, input_tokens: 1_500_120, output_tokens: 70_211, cache_read_tokens: 12_440_000, cache_creation_tokens: 380_000, uncached_input_tokens: 48_120, input_tps: 6_104.9, output_tps: 68.7, in_flight: 1, upstream_model: 'claude-sonnet-4-5', upstream: null },
      { route: 'claude-haiku', calls: 14, errors: 0, input_tokens: 250_000, output_tokens: 8_080, cache_read_tokens: 1_780_900, cache_creation_tokens: 22_000, uncached_input_tokens: 6_400, input_tps: 11_820, output_tps: 164.2, in_flight: 1, upstream_model: 'claude-haiku-4-5', upstream: null },
    ],
    kpi: {
      input_tokens: 4_760_562,
      output_tokens: 239_293,
      input_rate: 1_842.6 + ((now / 1000) % 7) * 31,
      output_rate: 41.8 + ((now / 1000) % 5),
      cache_read_tokens: 14_220_900,
      cache_creation_tokens: 402_000,
      uncached_input_tokens: 3_064_962,
      cache_hit_rate: 14_220_900 / (14_220_900 + 402_000 + 3_064_962),
      server_cache_hit_rate: 0.457,
      server_cache_hit_tokens: 1_376_200,
      route_rates: [
        { route: 'qwen-coder', input_tps: 1_204.3, output_tps: 28.1 },
        { route: 'claude-sonnet', input_tps: 638.3, output_tps: 13.7 },
        { route: 'claude-haiku', input_tps: null, output_tps: null },
      ],
    },
    limits: [
      {
        credential: '3fa9c1d2',
        seen_ms: now - 12_000,
        headers: {
          'anthropic-ratelimit-unified-status': 'allowed_warning',
          'anthropic-ratelimit-unified-5h-status': 'allowed_warning',
          'anthropic-ratelimit-unified-5h-utilization': '0.8',
          'anthropic-ratelimit-unified-5h-reset': String(RESET_5H_S),
          'anthropic-ratelimit-unified-7d-status': 'allowed',
          'anthropic-ratelimit-unified-7d-utilization': '0.41',
          'anthropic-ratelimit-unified-7d-reset': String(RESET_7D_S),
          'anthropic-ratelimit-unified-representative-claim': 'five_hour',
          'anthropic-ratelimit-unified-reset': String(RESET_5H_S),
          'anthropic-ratelimit-unified-overage-status': 'rejected',
          'anthropic-ratelimit-unified-overage-disabled-reason': 'out_of_credits',
          'anthropic-ratelimit-unified-fallback': 'available',
          'anthropic-ratelimit-unified-fallback-percentage': '0.5',
        },
      },
      {
        credential: 'b07e44a9',
        seen_ms: now - 340_000,
        headers: {
          'anthropic-ratelimit-requests-limit': '4000',
          'anthropic-ratelimit-requests-remaining': '3998',
          'anthropic-ratelimit-requests-reset': '2026-11-04T16:20:01Z',
          'anthropic-ratelimit-input-tokens-limit': '2000000',
          'anthropic-ratelimit-input-tokens-remaining': '1984000',
          'anthropic-ratelimit-output-tokens-limit': '400000',
          'anthropic-ratelimit-output-tokens-remaining': '399000',
        },
      },
    ],
    provider: {
      name: 'runpod',
      read_ms: now - 21_000,
      balance_usd: 87.412,
      spend_per_hour_usd: 2.39,
      other_pods: 1,
      pods: [
        { id: 'abc123xyz', name: 'b10x-llm-qwen-coder', model: 'Qwen/Qwen3-Coder-30B-A3B-Instruct', state: 'RUNNING', gpu: 'NVIDIA H100 80GB HBM3', cost_per_hour_usd: 2.39, uptime_s: 3 * 3600 + 1240, vllm_ready: true, last_call_ms: now - 2_400, in_flight: 1, shutdown: { at_ms: T0 + 17 * 60_000, limit: 'idle' }, external: false },
        { id: 'def456uvw', name: 'b10x-llm-small', model: null, state: 'STARTING', gpu: 'NVIDIA L40S', cost_per_hour_usd: 0.86, uptime_s: 95, vllm_ready: false, last_call_ms: null, in_flight: 0, shutdown: { at_ms: T0 + 4 * 60_000 + 30_000, limit: 'max-runtime' }, external: false },
        { id: 'ghi789rst', name: 'qwen-shared', model: 'qwen-coder', state: 'RUNNING', gpu: 'NVIDIA A100 80GB PCIe', cost_per_hour_usd: 1.64, uptime_s: 26 * 3600, vllm_ready: true, last_call_ms: now - 9_000, in_flight: 0, shutdown: null, external: true },
      ],
      watchdog_heartbeat_age_ms: 420_000,
    },
  };
}
