// Builders for hermetic scenario snapshots. Every id is synthetic hex; no value comes from a
// running gateway.
import type { CallRecord, InFlight, Kpi, LimitRow, ProviderPanel, RouteRow, SessionRow, Snapshot } from '../../src/extra.ts';

export interface RouteDef {
  route: string;
  upstream: 'anthropic' | 'runpod';
  model: string;
  wire: CallRecord['wire'];
}

export const ROUTES: Record<string, RouteDef> = {
  'qwen-coder': { route: 'qwen-coder', upstream: 'runpod', model: 'Qwen/Qwen3-Coder-30B-A3B-Instruct', wire: 'chat' },
  'claude-sonnet': { route: 'claude-sonnet', upstream: 'anthropic', model: 'claude-sonnet-4-5', wire: 'messages' },
  'claude-haiku': { route: 'claude-haiku', upstream: 'anthropic', model: 'claude-haiku-4-5', wire: 'messages' },
};

export interface SessionDef {
  session: string;
  label: string;
}

export interface CallSpec {
  session: SessionDef;
  route: string | null;
  at: number;
  input?: number;
  output?: number;
  cacheRead?: number;
  status?: number;
  outcome?: CallRecord['outcome'];
  refusal?: string;
  retryAfter?: string;
  attempts?: CallRecord['attempts'];
  durationMs?: number;
}

export const RATE_LIMITED =
  '{"type":"error","error":{"type":"rate_limit_error","message":"This request would exceed your rate limit."}}';

export function emptyKpi(): Kpi {
  return {
    input_tokens: 0,
    output_tokens: 0,
    input_rate: null,
    output_rate: null,
    cache_read_tokens: 0,
    cache_creation_tokens: 0,
    uncached_input_tokens: 0,
    cache_hit_rate: null,
    cache_routes: [],
    route_rates: [],
  };
}

export function snapshotAt(now: number, startedMs: number): Snapshot {
  return {
    format: 'llm-gateway.dashboard/1',
    now_ms: now,
    started_ms: startedMs,
    calls: [],
    in_flight: [],
    sessions: [],
    routes: [],
    provider: null,
    kpi: emptyKpi(),
    limits: [],
  };
}

// Accumulates calls and derives the session, route and overview rows from them, so every state of a
// replay is consistent with the calls it shows.
export class Ledger {
  private nextId = 5000;
  readonly calls: CallRecord[] = [];

  add(spec: CallSpec): CallRecord {
    const def = spec.route ? ROUTES[spec.route] : undefined;
    const outcome = spec.outcome ?? 'ok';
    const ok = outcome === 'ok';
    const duration = spec.durationMs ?? 1_800 + ((this.nextId * 977) % 9_000);
    const input = ok ? (spec.input ?? null) : null;
    const output = ok ? (spec.output ?? null) : null;
    const record: CallRecord = {
      id: this.nextId++,
      started_ms: spec.at,
      route: def ? def.route : null,
      upstream: def ? def.upstream : null,
      upstream_model: def ? def.model : null,
      wire: def ? def.wire : 'chat',
      status: spec.status ?? 200,
      target_status: def ? (spec.status ?? 200) : null,
      outcome,
      refusal: spec.refusal ?? null,
      queue_wait_ms: def?.upstream === 'runpod' ? 20 : null,
      ttft_ms: ok ? 240 + ((this.nextId * 131) % 900) : null,
      duration_ms: duration,
      response_bytes: ok ? 2_000 + (output ?? 0) * 4 : 180,
      input_tokens: input,
      output_tokens: output,
      cache_read_tokens: ok && def?.upstream === 'anthropic' ? (spec.cacheRead ?? 0) : null,
      cache_creation_tokens: ok && def?.upstream === 'anthropic' ? 0 : null,
      input_tps: input !== null ? Math.round((input / Math.max(1, duration)) * 1000) : null,
      output_tps: output !== null ? Math.round((output / Math.max(1, duration)) * 1000 * 10) / 10 : null,
      usage_note: null,
      client_retry: null,
      repeated_body: false,
      client_disconnected: false,
      upstream_retry_after: spec.retryAfter ?? null,
      attempts: spec.attempts ?? [],
      session: spec.session.session,
      user_agent: spec.session.label,
    };
    this.calls.unshift(record);
    return record;
  }

  sessions(): SessionRow[] {
    const rows = new Map<string, SessionRow>();
    for (const c of [...this.calls].reverse()) {
      const row = rows.get(c.session) ?? {
        session: c.session,
        label: c.user_agent ?? '',
        calls: 0,
        errors: 0,
        cancelled: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: null,
        input_tps: null,
        output_tps: null,
        last_ms: c.started_ms,
      };
      row.calls += 1;
      if (c.outcome !== 'ok') row.errors += 1;
      row.input_tokens += c.input_tokens ?? 0;
      row.output_tokens += c.output_tokens ?? 0;
      if (c.cache_read_tokens !== null) row.cache_read_tokens = (row.cache_read_tokens ?? 0) + c.cache_read_tokens;
      if (c.input_tps !== null) row.input_tps = c.input_tps;
      if (c.output_tps !== null) row.output_tps = c.output_tps;
      row.last_ms = Math.max(row.last_ms, c.started_ms);
      rows.set(c.session, row);
    }
    return [...rows.values()].sort((a, b) => b.last_ms - a.last_ms);
  }

  routes(inFlight: InFlight[]): RouteRow[] {
    const rows = new Map<string, RouteRow>();
    for (const c of this.calls) {
      if (!c.route) continue;
      const def = ROUTES[c.route]!;
      const row = rows.get(c.route) ?? {
        route: c.route,
        calls: 0,
        errors: 0,
        cancelled: 0,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: def.upstream === 'anthropic' ? 0 : null,
        uncached_input_tokens: 0,
        cache_hit_rate: null,
        cache_source: def.upstream === 'runpod' ? 'vllm' : 'calls',
        input_tps: null,
        output_tps: null,
        in_flight: 0,
        upstream_model: def.model,
        upstream: null,
      };
      row.calls += 1;
      if (c.outcome !== 'ok') row.errors += 1;
      row.input_tokens += c.input_tokens ?? 0;
      row.output_tokens += c.output_tokens ?? 0;
      row.cache_read_tokens += c.cache_read_tokens ?? 0;
      row.uncached_input_tokens += c.input_tokens ?? 0;
      const prompt = row.cache_read_tokens + row.uncached_input_tokens;
      row.cache_hit_rate = prompt > 0 ? row.cache_read_tokens / prompt : null;
      if (c.input_tps !== null && row.input_tps === null) row.input_tps = c.input_tps;
      if (c.output_tps !== null && row.output_tps === null) row.output_tps = c.output_tps;
      rows.set(c.route, row);
    }
    for (const f of inFlight) {
      const row = f.route ? rows.get(f.route) : undefined;
      if (row) row.in_flight += 1;
    }
    return [...rows.values()];
  }

  kpi(now: number, routes: RouteRow[]): Kpi {
    const kpi = emptyKpi();
    for (const r of routes) {
      kpi.input_tokens += r.input_tokens;
      kpi.output_tokens += r.output_tokens;
      kpi.cache_read_tokens += r.cache_read_tokens;
      kpi.uncached_input_tokens += r.uncached_input_tokens;
    }
    const prompt = kpi.cache_read_tokens + kpi.uncached_input_tokens;
    kpi.cache_hit_rate = prompt > 0 ? kpi.cache_read_tokens / prompt : null;
    const recent = this.calls.filter((c) => now - c.started_ms <= 60_000);
    if (recent.length > 0) {
      kpi.input_rate = recent.reduce((a, c) => a + (c.input_tokens ?? 0), 0) / 60;
      kpi.output_rate = recent.reduce((a, c) => a + (c.output_tokens ?? 0), 0) / 60;
    }
    kpi.cache_routes = routes
      .map((r) => ({
        route: r.route,
        source: r.cache_source,
        hit_rate: r.cache_hit_rate,
        read_tokens: r.cache_read_tokens,
        prompt_tokens: r.cache_read_tokens + r.uncached_input_tokens,
      }))
      .sort((a, b) => a.route.localeCompare(b.route));
    kpi.route_rates = routes.map((r) => {
      const mine = recent.filter((c) => c.route === r.route);
      return {
        route: r.route,
        input_tps: mine.length ? mine.reduce((a, c) => a + (c.input_tokens ?? 0), 0) / 60 : null,
        output_tps: mine.length ? mine.reduce((a, c) => a + (c.output_tokens ?? 0), 0) / 60 : null,
      };
    });
    return kpi;
  }

  snapshot(now: number, startedMs: number, extra: { inFlight?: InFlight[]; provider?: ProviderPanel | null; limits?: LimitRow[] } = {}): Snapshot {
    const inFlight = extra.inFlight ?? [];
    const routes = this.routes(inFlight);
    return {
      ...snapshotAt(now, startedMs),
      calls: this.calls.map((c) => ({ ...c, attempts: c.attempts.map((a) => ({ ...a })) })),
      in_flight: inFlight,
      sessions: this.sessions(),
      routes,
      kpi: this.kpi(now, routes),
      provider: extra.provider ?? null,
      limits: extra.limits ?? [],
    };
  }
}

export function inFlight(id: number, session: SessionDef, route: string, now: number, startedMs: number, state: InFlight['state'] = 'streaming'): InFlight {
  const def = ROUTES[route]!;
  return {
    id,
    started_ms: startedMs,
    route,
    upstream_model: def.model,
    wire: def.wire,
    state,
    attempt: 1,
    retry_at_ms: null,
    elapsed_ms: now - startedMs,
    session: session.session,
  };
}
