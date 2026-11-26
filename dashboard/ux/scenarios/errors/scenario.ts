import type { AttemptRecord, LimitRow, ProviderPanel, Snapshot } from '../../../src/extra.ts';
import { Ledger, RATE_LIMITED, inFlight, type SessionDef } from '../build.ts';
import type { Scenario } from '../types.ts';

const A: SessionDef = { session: '9c4e01d7b3a2', label: 'claude-code' };
const B: SessionDef = { session: '27fa6b0e5c19', label: 'curl/8.11' };

function limited(): AttemptRecord[] {
  return [
    { attempt: 1, status: 429, duration_ms: 290, wait_ms: 0, retry_after: '30', headers: {}, error_body: RATE_LIMITED },
    { attempt: 2, status: 429, duration_ms: 310, wait_ms: 30_000, retry_after: '30', headers: {}, error_body: RATE_LIMITED },
  ];
}

function provider(now: number, shutdownAt: number): ProviderPanel {
  return {
    name: 'runpod',
    read_ms: now - 9_000,
    balance_usd: 3.208,
    spend_per_hour_usd: 2.39,
    other_pods: 0,
    pods: [
      {
        id: 'f0e1d2c3b4a5',
        name: 'b10x-llm-qwen-coder',
        model: 'Qwen/Qwen3-Coder-30B-A3B-Instruct',
        state: 'RUNNING',
        gpu: 'NVIDIA H100 80GB HBM3',
        cost_per_hour_usd: 2.39,
        uptime_s: 2 * 3600 + 610,
        vllm_ready: true,
        last_call_ms: now - 48_000,
        in_flight: 0,
        shutdown: { at_ms: shutdownAt, limit: 'balance' },
        external: false,
      },
    ],
    // Older than the page's five-minute staleness bound.
    watchdog_heartbeat_age_ms: 11 * 60_000,
  };
}

function limits(now: number): LimitRow[] {
  const reset = String(Math.floor((now + 47 * 60_000) / 1000));
  return [
    {
      credential: '6b1d9e40',
      seen_ms: now - 3_000,
      headers: {
        'anthropic-ratelimit-unified-status': 'rejected',
        'anthropic-ratelimit-unified-5h-status': 'rejected',
        'anthropic-ratelimit-unified-5h-utilization': '1.0',
        'anthropic-ratelimit-unified-5h-reset': reset,
        'anthropic-ratelimit-unified-7d-status': 'allowed_warning',
        'anthropic-ratelimit-unified-7d-utilization': '0.86',
        'anthropic-ratelimit-unified-7d-reset': String(Math.floor((now + 2 * 86_400_000) / 1000)),
        'anthropic-ratelimit-unified-representative-claim': 'five_hour',
        'anthropic-ratelimit-unified-reset': reset,
        'anthropic-ratelimit-unified-overage-status': 'rejected',
        'anthropic-ratelimit-unified-overage-disabled-reason': 'org_level_disabled',
      },
    },
  ];
}

// A gateway in trouble: refusals for an unknown model and a bad credential, 429s with retries, a
// call waiting for its retry, a stale watchdog, and the one pod shutting down within a minute on
// the balance floor. The replay ends by opening the attempts of the first failed call.
export const errors: Scenario = {
  name: 'errors',
  description: 'Refusals, 429s with retries, a stale watchdog, a pod shutting down within a minute.',
  anchor: true,
  states(base: number): Snapshot[] {
    const ledger = new Ledger();
    const started = base - 2 * 3600_000;
    const shutdownAt = base + 70_000;
    const out: Snapshot[] = [];
    for (let i = 0; i < 4; i++) {
      const now = base + i * 5_000;
      ledger.add({ session: B, route: null, at: now - 1_200, status: 401, outcome: 'refused', refusal: 'unknown credential', durationMs: 1 });
      ledger.add({ session: A, route: 'claude-sonnet', at: now - 900, status: 429, outcome: 'upstream_error', retryAfter: '30', attempts: limited(), durationMs: 30_600 });
      if (i % 2 === 1) ledger.add({ session: B, route: null, at: now - 400, status: 404, outcome: 'refused', refusal: 'unknown model', durationMs: 1 });
      if (i === 2) ledger.add({ session: A, route: 'qwen-coder', at: now - 700, status: 504, outcome: 'timeout', durationMs: 600_000 });
      const retry = { ...inFlight(9100 + i, A, 'claude-haiku', now, now - 6_000, 'waiting-retry'), attempt: 2, retry_at_ms: now + 24_000 };
      out.push(ledger.snapshot(now, started, { inFlight: [retry], provider: provider(now, shutdownAt), limits: limits(now) }));
    }
    return out;
  },
  actions: ['expand-first-attempts'],
};
