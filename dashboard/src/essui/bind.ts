// The binding: answers each placeholder view of ui/llm-gateway.ui.yaml from the gateway's one
// snapshot (`dashboard/snapshot` and the `snapshot` events of `dashboard/events`), at the gateway's
// clock `now`. There is no ESS model of these views; this file is what they mean. Each answer's
// rows carry the fields the document's columns, fields and metrics read, worded as the page shows
// them where ess-ui/1 has no rendering for it (see Rich in types.ts).
import { DASH, ago, clock, count, countdown, ms, pct, span, tps, usd } from '../format.ts';
import type { CallRecord, InFlight, LimitRow, PodRow, Snapshot } from '../extra.ts';
import type { Part, Rich, Row } from './types.ts';

export type Params = Record<string, unknown>;
export type Answer = (snap: Snapshot, now: number, params: Params) => Row[];

export const WATCHDOG_STALE_MS = 5 * 60_000;
const SHUTDOWN_SOON_MS = 5 * 60_000;
const SHUTDOWN_LIMIT: Record<string, string> = { idle: 'idle limit', 'max-runtime': 'max runtime', balance: 'balance floor' };
const VLLM_WRITE = 'vLLM stores computed blocks automatically; not reported separately';
const VLLM_SOURCE = "from the vLLM server's prefix-cache counters";
const CACHE_ABOUT = "cache read / every prompt token, over every route: Anthropic's reported cache, and a vLLM server's prefix-cache hits over its queries";
const UNIFIED = 'anthropic-ratelimit-unified-';
const IN_TILES = ['status', 'reset', 'representative-claim', 'overage-status', 'overage-disabled-reason', '5h-status', '5h-utilization', '5h-reset', '7d-status', '7d-utilization', '7d-reset'].map((k) => UNIFIED + k);

const pair = (a: number | null, b: number | null) => `${count(a)} / ${count(b)}`;
const yesNo = (v: boolean | null) => (v === null ? DASH : v ? 'yes' : 'no');
const parts = (...lines: (string | null | undefined | false)[]): Part[] => lines.filter((l): l is string => !!l).map((text) => ({ text }));

function unixMs(v: string | undefined): number | null {
  if (v === undefined) return null;
  const n = Number(v);
  return Number.isFinite(n) ? n * 1000 : null;
}

function fraction(v: string | undefined): number | null {
  if (v === undefined) return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
}

// The class of a call's answer the status pill is toned by.
export function callResult(c: Pick<CallRecord, 'outcome' | 'status'>): 'ok' | 'refused' | 'failed' {
  if (c.outcome === 'ok' && c.status < 400) return 'ok';
  return c.outcome === 'refused' ? 'refused' : 'failed';
}

export function flags(c: CallRecord): string[] {
  const out: string[] = [];
  if (c.client_retry !== null && c.client_retry > 0) out.push(`retry ${c.client_retry}`);
  if (c.repeated_body) out.push('repeated body');
  if (c.client_disconnected) out.push('client gone');
  if (c.upstream_retry_after !== null) out.push(`retry-after ${c.upstream_retry_after}`);
  if (c.refusal !== null) out.push(c.refusal);
  if (c.usage_note) out.push(c.usage_note);
  return out;
}

function inflightState(f: InFlight, now: number): string {
  if (f.state === 'waiting-retry' && f.retry_at_ms !== null && f.retry_at_ms !== undefined) {
    const left = Math.ceil((f.retry_at_ms - now) / 1000);
    return left > 0 ? `waiting for retry, ${left} s left` : 'retrying now';
  }
  return f.state;
}

function shutdown(p: PodRow, now: number): Rich {
  if (p.external) return { text: 'external, not stopped by this gateway', muted: true };
  if (!p.shutdown) return { text: DASH };
  const soon = p.shutdown.at_ms - now < SHUTDOWN_SOON_MS;
  return { text: `${countdown(now, p.shutdown.at_ms)} (${SHUTDOWN_LIMIT[p.shutdown.limit] ?? p.shutdown.limit})`, key: soon ? 'soon' : 'later' };
}

function heartbeat(age: number | null | undefined): Rich {
  if (age === null || age === undefined) return { text: 'none', key: 'none' };
  const stale = age > WATCHDOG_STALE_MS;
  const mins = WATCHDOG_STALE_MS / 60_000;
  return stale
    ? { text: `${span(age)} ago · stale (over ${mins} min)`, key: 'stale', title: `no heartbeat for over ${mins} min: pods may not be stopped on schedule` }
    : { text: `${span(age)} ago`, key: 'fresh' };
}

function limitRow(row: LimitRow, now: number): Row {
  const h: Record<string, string> = {};
  for (const [k, v] of Object.entries(row.headers)) h[k.toLowerCase()] = v;
  const u = (k: string) => h[UNIFIED + k];
  const window = (w: '5h' | '7d'): Rich | null => {
    if (u(`${w}-utilization`) === undefined && u(`${w}-status`) === undefined) return null;
    return { text: pct(fraction(u(`${w}-utilization`))), key: u(`${w}-status`) ?? '', sub: parts(`resets ${countdown(now, unixMs(u(`${w}-reset`)))}`) };
  };
  const status = u('status');
  const reset = unixMs(u('reset'));
  const overage = u('overage-status');
  const reason = u('overage-disabled-reason');
  const claim = u('representative-claim');
  const rest = Object.entries(h)
    .filter(([k]) => !IN_TILES.includes(k))
    .sort(([a], [b]) => a.localeCompare(b));
  return {
    credential: { text: row.credential, mono: true, note: { text: `seen ${ago(now, row.seen_ms)}` } },
    status: { text: status ?? DASH, title: status, sub: reset !== null ? parts(`resets ${countdown(now, reset)}`) : [] },
    window_5h: window('5h'),
    window_7d: window('7d'),
    overage: overage === undefined ? null : { text: overage, title: overage, sub: reason ? [{ text: reason, title: reason }] : [] },
    claim: claim === undefined ? null : { text: claim, title: claim },
    headers: rest.length > 0 ? Object.fromEntries(rest) : null,
  };
}

function callRow(c: CallRecord, now: number): Row {
  return {
    id: c.id,
    time: { text: clock(c.started_ms), mono: true, title: ago(now, c.started_ms) },
    route: { text: c.route ?? DASH, sub: parts(c.upstream_model !== c.route && c.upstream_model, c.upstream) },
    wire: c.wire,
    status: { text: String(c.status), key: callResult(c), title: c.outcome },
    ttft: ms(c.ttft_ms),
    duration: ms(c.duration_ms),
    queue: ms(c.queue_wait_ms),
    tokens: pair(c.input_tokens, c.output_tokens),
    cache: pair(c.cache_read_tokens, c.cache_creation_tokens),
    input_tps: tps(c.input_tps),
    output_tps: tps(c.output_tps),
    flags: flags(c),
    attempts: c.attempts.length,
  };
}

// Every placeholder the document reads, by name.
export const BINDINGS: Record<string, Answer> = {
  'dashboard.Gateway': (s, now) => [{ uptime_ms: now - s.started_ms }],

  // The host component draws the token flow from the whole snapshot.
  'dashboard.TokenFlow': (s) => [s as unknown as Row],

  'dashboard.Overview': (s) => {
    const k = s.kpi;
    const p = s.provider;
    return [
      {
        input_tokens: { text: count(k.input_tokens), sub: parts(tps(k.input_rate)) },
        output_tokens: { text: count(k.output_tokens), sub: parts(tps(k.output_rate)) },
        cache_hit_rate: {
          text: pct(k.cache_hit_rate),
          about: CACHE_ABOUT,
          sub: parts(`read ${count(k.cache_read_tokens)} · created ${count(k.cache_creation_tokens)} · uncached ${count(k.uncached_input_tokens)}`),
          table: k.cache_routes.map((r) => [{ text: r.route, tag: r.source === 'vllm' ? { text: 'vLLM' } : undefined }, { text: pct(r.hit_rate) }, { text: `${count(r.read_tokens)} / ${count(r.prompt_tokens)}` }]),
        },
        throughput: { text: '', table: k.route_rates.map((r) => [{ text: r.route }, { text: `in ${tps(r.input_tps)}` }, { text: `out ${tps(r.output_tps)}` }]), empty: 'Idle.' },
        balance: p ? { text: usd(p.balance_usd), sub: parts(`${usd(p.spend_per_hour_usd)}/h spend`) } : null,
      },
    ];
  },

  'dashboard.Limits': (s, now) => (s.limits ?? []).map((l) => limitRow(l, now)),

  'dashboard.Provider': (s, now) => {
    const p = s.provider;
    if (!p) return [];
    return [{ name: p.name, other_pods: p.other_pods, heartbeat: heartbeat(p.watchdog_heartbeat_age_ms), read: ago(now, p.read_ms) }];
  },

  'dashboard.Pods': (s, now) =>
    (s.provider?.pods ?? []).map((p) => ({
      id: { text: p.id, mono: true, sub: parts(p.name) },
      model: { text: p.model ?? DASH, wrap: true },
      state: p.state,
      gpu: p.gpu ?? DASH,
      cost_per_hour: usd(p.cost_per_hour_usd),
      uptime: p.uptime_s === null ? DASH : span(p.uptime_s * 1000),
      vllm_ready: yesNo(p.vllm_ready),
      last_call: ago(now, p.last_call_ms),
      in_flight: p.in_flight,
      shutdown: shutdown(p, now),
    })),

  'dashboard.InFlight': (s, now) =>
    s.in_flight.map((f) => ({
      id: f.id,
      route: f.route ?? DASH,
      model: { text: f.upstream_model ?? DASH, mono: true },
      wire: f.wire,
      state: { text: inflightState(f, now), key: f.state, note: f.attempt > 1 ? { text: `attempt ${f.attempt}` } : undefined },
      elapsed: ms(f.elapsed_ms),
      session: { text: f.session, mono: true },
    })),

  'dashboard.Routes': (s) =>
    s.routes.map((r) => ({
      route: { text: r.route, sub: parts(r.upstream_model !== r.route && r.upstream_model) },
      calls: r.calls,
      errors: r.errors,
      cancelled: r.cancelled,
      tokens: pair(r.input_tokens, r.output_tokens),
      cache:
        r.cache_source === 'vllm'
          ? { text: pair(r.cache_read_tokens, r.cache_creation_tokens), title: VLLM_WRITE, tag: { text: 'vLLM', title: VLLM_SOURCE } }
          : pair(r.cache_read_tokens, r.cache_creation_tokens),
      uncached: r.uncached_input_tokens,
      cache_hit: r.cache_hit_rate,
      input_tps: tps(r.input_tps),
      output_tps: tps(r.output_tps),
      in_flight: r.in_flight,
      server: r.upstream ? `${count(r.upstream.requests_running)} / ${count(r.upstream.requests_waiting)}` : DASH,
      kv_cache: r.upstream ? r.upstream.kv_cache_usage : null,
    })),

  'dashboard.Sessions': (s, now) =>
    s.sessions.map((r) => ({
      session: { text: r.label, note: { text: r.session, mono: true } },
      calls: r.calls,
      errors: r.errors,
      cancelled: r.cancelled,
      tokens: pair(r.input_tokens, r.output_tokens),
      cache_read: r.cache_read_tokens,
      input_tps: tps(r.input_tps),
      output_tps: tps(r.output_tps),
      last_seen: ago(now, r.last_ms),
    })),

  'dashboard.Calls': (s, now) => s.calls.map((c) => callRow(c, now)),

  'dashboard.Attempts': (s, _now, params) =>
    (s.calls.find((c) => c.id === params.call)?.attempts ?? []).map((a) => ({
      call: params.call as number,
      attempt: a.attempt,
      status: { text: a.status === null ? 'no answer' : String(a.status), key: a.status !== null && a.status < 400 ? 'ok' : 'failed' },
      duration: ms(a.duration_ms),
      wait: ms(a.wait_ms),
      retry_after: a.retry_after ?? DASH,
      error_body: a.error_body ?? DASH,
    })),
};

export function answer(placeholder: string, snap: Snapshot, now: number, params: Params = {}): Row[] {
  const bind = BINDINGS[placeholder];
  if (!bind) throw new Error(`no binding answers the placeholder ${placeholder}`);
  return bind(snap, now, params);
}
