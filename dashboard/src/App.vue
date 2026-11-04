<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import { useFeed, type Link } from './feed.ts';
import { DASH, ago, clock, count, countdown, ms, pct, span, tps, usd } from './format.ts';
import type { CallRecord, InFlight, PodRow, Snapshot } from './extra.ts';
import Kpis from './Kpis.vue';

const WATCHDOG_STALE_MS = 5 * 60_000;
const SHUTDOWN_LIMIT: Record<string, string> = { idle: 'idle limit', 'max-runtime': 'max runtime', balance: 'balance floor' };

const feed = useFeed();
const fixtureSnapshot = ref<Snapshot | null>(null);

const snap = computed<Snapshot | null>(() => fixtureSnapshot.value ?? feed.snapshot.value);
const link = computed<Link>(() => (fixtureSnapshot.value ? 'live' : feed.link.value));

// Countdowns tick every second on the gateway's clock: the offset is taken from each snapshot.
const tick = ref(Date.now());
const offset = ref(0);
watch(snap, (s) => (offset.value = s ? s.now_ms - Date.now() : 0), { immediate: true });
const timer = setInterval(() => (tick.value = Date.now()), 1000);
onUnmounted(() => clearInterval(timer));
const now = computed(() => tick.value + offset.value);

const open = reactive(new Set<number>());
function toggle(id: number) {
  if (open.has(id)) open.delete(id);
  else open.add(id);
}

const showKpis = computed(() => !!snap.value && (!!snap.value.kpi || !!snap.value.limits?.length || !!snap.value.provider));

function inflightState(f: InFlight): string {
  if (f.state === 'waiting-retry' && f.retry_at_ms !== null && f.retry_at_ms !== undefined) {
    const left = Math.ceil((f.retry_at_ms - now.value) / 1000);
    return left > 0 ? `waiting for retry, ${left} s left` : 'retrying now';
  }
  return f.state;
}

function shutdown(p: PodRow): string {
  if (!p.shutdown) return DASH;
  return `${countdown(now.value, p.shutdown.at_ms)} (${SHUTDOWN_LIMIT[p.shutdown.limit] ?? p.shutdown.limit})`;
}

function shutdownSoon(p: PodRow): boolean {
  return !!p.shutdown && p.shutdown.at_ms - now.value < 5 * 60_000;
}

const LINK_TEXT: Record<Link, string> = {
  connecting: 'connecting',
  live: 'live',
  polling: 'polling every 5 s',
  offline: 'offline',
};

onMounted(async () => {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has('fixture')) {
    const { fixture } = await import('./fixture.ts');
    fixtureSnapshot.value = fixture(Date.now());
    setInterval(() => (fixtureSnapshot.value = fixture(Date.now())), 1000);
    return;
  }
  await feed.start();
});

const watchdogStale = computed(() => {
  const age = snap.value?.provider?.watchdog_heartbeat_age_ms;
  return age === null || age === undefined || age > WATCHDOG_STALE_MS;
});

function flags(c: CallRecord): string[] {
  const out: string[] = [];
  if (c.client_retry !== null && c.client_retry > 0) out.push(`retry ${c.client_retry}`);
  if (c.repeated_body) out.push('repeated body');
  if (c.client_disconnected) out.push('client gone');
  if (c.upstream_retry_after !== null) out.push(`retry-after ${c.upstream_retry_after}`);
  if (c.refusal !== null) out.push(c.refusal);
  if (c.usage_note) out.push(c.usage_note);
  return out;
}

function statusClass(c: CallRecord): string {
  if (c.outcome === 'ok' && c.status < 400) return 'ok';
  return c.outcome === 'refused' ? 'warn' : 'bad';
}

function tokenPair(a: number | null, b: number | null): string {
  return `${count(a)} / ${count(b)}`;
}

function yesNo(v: boolean | null): string {
  return v === null ? DASH : v ? 'yes' : 'no';
}
</script>

<template>
  <header class="top">
    <h1>llm-gateway</h1>
    <nav class="nav"><a href="/">Setup</a></nav>
    <span v-if="snap" class="muted">up {{ span(now - snap.started_ms) }}</span>
    <span class="link" :class="link" role="status" aria-live="polite">
      <span class="dot" aria-hidden="true"></span>{{ LINK_TEXT[link] }}
    </span>
  </header>

  <main v-if="snap">
    <Kpis v-if="showKpis" :snap="snap" :now="now" />

    <section v-if="snap.provider" aria-labelledby="h-provider">
      <h2 id="h-provider">Provider: {{ snap.provider.name }}</h2>
      <dl class="stats">
        <div><dt>Other pods</dt><dd>{{ snap.provider.other_pods }}</dd></div>
        <div :class="{ alarm: watchdogStale }">
          <dt>Watchdog heartbeat</dt>
          <dd>{{ snap.provider.watchdog_heartbeat_age_ms === null ? 'none' : span(snap.provider.watchdog_heartbeat_age_ms) + ' ago' }}</dd>
        </div>
        <div><dt>Read</dt><dd>{{ ago(now, snap.provider.read_ms) }}</dd></div>
      </dl>
      <div class="scroll">
        <table>
          <caption>Owned pods</caption>
          <thead>
            <tr>
              <th scope="col">Pod</th><th scope="col">Model</th><th scope="col">State</th><th scope="col">GPU</th>
              <th scope="col" class="n">$/h</th><th scope="col" class="n">Uptime</th><th scope="col">vLLM ready</th>
              <th scope="col" class="n">Last call</th><th scope="col" class="n">In flight</th><th scope="col">Automatic shutdown</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="p in snap.provider.pods" :key="p.id">
              <th scope="row"><span class="mono">{{ p.id }}</span><div class="muted small">{{ p.name }}</div></th>
              <td class="wrap">{{ p.model ?? DASH }}</td>
              <td><span class="pill" :class="p.state === 'RUNNING' ? 'ok' : 'warn'">{{ p.state }}</span></td>
              <td>{{ p.gpu ?? DASH }}</td>
              <td class="n">{{ usd(p.cost_per_hour_usd) }}</td>
              <td class="n">{{ p.uptime_s === null ? DASH : span(p.uptime_s * 1000) }}</td>
              <td :class="p.vllm_ready ? 'ok-text' : p.vllm_ready === false ? 'warn-text' : ''">{{ yesNo(p.vllm_ready) }}</td>
              <td class="n">{{ ago(now, p.last_call_ms) }}</td>
              <td class="n">{{ p.in_flight }}</td>
              <td v-if="p.external" class="muted">external, not stopped by this gateway</td>
              <td v-else :class="{ 'warn-text': shutdownSoon(p) }">{{ shutdown(p) }}</td>
            </tr>
            <tr v-if="snap.provider.pods.length === 0"><td colspan="10" class="empty">No pods.</td></tr>
          </tbody>
        </table>
      </div>
    </section>

    <div class="grid">
      <section aria-labelledby="h-inflight">
        <h2 id="h-inflight">In flight <span class="badge">{{ snap.in_flight.length }}</span></h2>
        <div class="scroll">
          <table>
            <thead>
              <tr><th scope="col">Route</th><th scope="col">Wire</th><th scope="col">State</th><th scope="col" class="n">Elapsed</th><th scope="col">Session</th></tr>
            </thead>
            <tbody>
              <tr v-for="f in snap.in_flight" :key="f.id">
                <td>{{ f.route ?? DASH }}</td>
                <td>{{ f.wire }}</td>
                <td>
                  <span class="pill" :class="f.state === 'streaming' ? 'ok' : 'warn'">{{ inflightState(f) }}</span>
                  <span v-if="f.attempt > 1" class="muted small"> attempt {{ f.attempt }}</span>
                </td>
                <td class="n">{{ ms(f.elapsed_ms) }}</td>
                <td class="mono">{{ f.session }}</td>
              </tr>
              <tr v-if="snap.in_flight.length === 0"><td colspan="5" class="empty">Idle.</td></tr>
            </tbody>
          </table>
        </div>
      </section>

      <section aria-labelledby="h-routes">
        <h2 id="h-routes">Routes</h2>
        <div class="scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Route</th><th scope="col" class="n">Calls</th><th scope="col" class="n">Errors</th>
                <th scope="col" class="n">Tokens in / out</th><th scope="col" class="n">Cache r / w</th><th scope="col" class="n">Uncached in</th><th scope="col" class="n"><abbr title="cache read / every prompt token; vLLM: the server's prefix-cache hits over its queries, in tokens, since the gateway began reading">Cache hit</abbr></th>
                <th scope="col" class="n"><abbr title="average over the route's calls">In tok/s</abbr></th><th scope="col" class="n"><abbr title="average over the route's calls">Out tok/s</abbr></th>
                <th scope="col" class="n">In flight</th>
                <th scope="col" class="n"><abbr title="requests running / waiting at the vLLM server">Server run / wait</abbr></th>
                <th scope="col" class="n"><abbr title="share of the vLLM server's KV cache in use">KV cache</abbr></th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="r in snap.routes" :key="r.route">
                <th scope="row">{{ r.route }}<div v-if="r.upstream_model && r.upstream_model !== r.route" class="muted small">{{ r.upstream_model }}</div></th>
                <td class="n">{{ count(r.calls) }}</td>
                <td class="n" :class="{ 'bad-text': r.errors > 0 }">{{ count(r.errors) }}</td>
                <td class="n">{{ tokenPair(r.input_tokens, r.output_tokens) }}</td>
                <td class="n">{{ tokenPair(r.cache_read_tokens, r.cache_creation_tokens) }} <span v-if="r.cache_source === 'vllm'" class="badge" title="from the vLLM server's prefix-cache counters">vLLM</span></td>
                <td class="n">{{ count(r.uncached_input_tokens) }}</td>
                <td class="n">{{ pct(r.cache_hit_rate) }}</td>
                <td class="n">{{ tps(r.input_tps) }}</td>
                <td class="n">{{ tps(r.output_tps) }}</td>
                <td class="n">{{ r.in_flight }}</td>
                <td class="n">{{ r.upstream ? `${count(r.upstream.requests_running)} / ${count(r.upstream.requests_waiting)}` : DASH }}</td>
                <td class="n">{{ r.upstream ? pct(r.upstream.kv_cache_usage) : DASH }}</td>
              </tr>
              <tr v-if="snap.routes.length === 0"><td colspan="12" class="empty">No routes yet.</td></tr>
            </tbody>
          </table>
        </div>
      </section>
    </div>

    <section aria-labelledby="h-sessions">
      <h2 id="h-sessions">Sessions</h2>
      <div class="scroll">
        <table>
          <thead>
            <tr>
              <th scope="col">Session</th><th scope="col" class="n">Calls</th><th scope="col" class="n">Tokens in / out</th>
              <th scope="col" class="n">Cache read</th><th scope="col" class="n"><abbr title="input tokens per second until the first token">In tok/s</abbr></th>
              <th scope="col" class="n"><abbr title="output tokens per second after the first token">Out tok/s</abbr></th><th scope="col" class="n">Last seen</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="s in snap.sessions" :key="s.session">
              <th scope="row">{{ s.label }} <span class="mono muted small">{{ s.session }}</span></th>
              <td class="n">{{ count(s.calls) }}</td>
              <td class="n">{{ tokenPair(s.input_tokens, s.output_tokens) }}</td>
              <td class="n">{{ count(s.cache_read_tokens) }}</td>
              <td class="n">{{ tps(s.input_tps) }}</td>
              <td class="n">{{ tps(s.output_tps) }}</td>
              <td class="n">{{ ago(now, s.last_ms) }}</td>
            </tr>
            <tr v-if="snap.sessions.length === 0"><td colspan="7" class="empty">No sessions yet.</td></tr>
          </tbody>
        </table>
      </div>
    </section>

    <section aria-labelledby="h-calls">
      <h2 id="h-calls">Recent calls <span class="badge">{{ snap.calls.length }}</span></h2>
      <div class="scroll">
        <table class="calls">
          <thead>
            <tr>
              <th scope="col">Time</th><th scope="col">Route</th><th scope="col">Wire</th><th scope="col" class="n">Status</th>
              <th scope="col" class="n"><abbr title="time to first token">TTFT</abbr></th><th scope="col" class="n">Duration</th>
              <th scope="col" class="n">Queue</th><th scope="col" class="n">Tokens in / out</th><th scope="col" class="n">Cache r / w</th>
              <th scope="col" class="n">In tok/s</th><th scope="col" class="n">Out tok/s</th>
              <th scope="col">Flags</th><th scope="col">Attempts</th>
            </tr>
          </thead>
          <tbody>
            <template v-for="c in snap.calls" :key="c.id">
            <tr>
              <td class="mono" :title="ago(now, c.started_ms)">{{ clock(c.started_ms) }}</td>
              <td>{{ c.route ?? DASH }}<div v-if="c.upstream_model && c.upstream_model !== c.route" class="muted small">{{ c.upstream_model }}</div><div v-if="c.upstream" class="muted small">{{ c.upstream }}</div></td>
              <td>{{ c.wire }}</td>
              <td class="n"><span class="pill" :class="statusClass(c)" :title="c.outcome">{{ c.status }}</span></td>
              <td class="n">{{ ms(c.ttft_ms) }}</td>
              <td class="n">{{ ms(c.duration_ms) }}</td>
              <td class="n">{{ ms(c.queue_wait_ms) }}</td>
              <td class="n">{{ tokenPair(c.input_tokens, c.output_tokens) }}</td>
              <td class="n">{{ tokenPair(c.cache_read_tokens, c.cache_creation_tokens) }}</td>
              <td class="n">{{ tps(c.input_tps) }}</td>
              <td class="n">{{ tps(c.output_tps) }}</td>
              <td><span v-for="f in flags(c)" :key="f" class="flag">{{ f }}</span></td>
              <td>
                <button v-if="c.attempts.length" type="button" class="expand" :aria-expanded="open.has(c.id)" @click="toggle(c.id)">
                  {{ open.has(c.id) ? '▾' : '▸' }} {{ c.attempts.length }}
                </button>
                <span v-else class="muted">{{ DASH }}</span>
              </td>
            </tr>
            <tr v-if="open.has(c.id)" class="attempts">
              <td colspan="13">
                <table class="mini">
                  <thead>
                    <tr>
                      <th scope="col" class="n">Attempt</th><th scope="col" class="n">Status</th><th scope="col" class="n">Duration</th>
                      <th scope="col" class="n">Wait before</th><th scope="col">Retry-After</th><th scope="col">Error body</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr v-for="a in c.attempts" :key="a.attempt">
                      <td class="n">{{ a.attempt }}</td>
                      <td class="n"><span class="pill" :class="a.status !== null && a.status < 400 ? 'ok' : 'bad'">{{ a.status ?? 'no answer' }}</span></td>
                      <td class="n">{{ ms(a.duration_ms) }}</td>
                      <td class="n">{{ ms(a.wait_ms) }}</td>
                      <td>{{ a.retry_after ?? DASH }}</td>
                      <td class="wrap mono">{{ a.error_body ?? DASH }}</td>
                    </tr>
                  </tbody>
                </table>
              </td>
            </tr>
            </template>
            <tr v-if="snap.calls.length === 0"><td colspan="13" class="empty">No calls yet.</td></tr>
          </tbody>
        </table>
      </div>
    </section>
  </main>

  <main v-else class="waiting">
    <p>{{ feed.error.value ? `Cannot reach the gateway: ${feed.error.value}` : 'Waiting for the first snapshot…' }}</p>
  </main>
</template>
