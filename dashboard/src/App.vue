<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useFeed, type Link } from './feed.ts';
import { DASH, ago, clock, count, ms, rate, span, usd } from './format.ts';
import type { CallRecord, Snapshot } from './generated/snapshot.ts';

const WATCHDOG_STALE_MS = 5 * 60_000;

const feed = useFeed();
const fixtureSnapshot = ref<Snapshot | null>(null);

const snap = computed<Snapshot | null>(() => fixtureSnapshot.value ?? feed.snapshot.value);
const link = computed<Link>(() => (fixtureSnapshot.value ? 'live' : feed.link.value));
const now = computed(() => snap.value?.now_ms ?? Date.now());

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
    <span v-if="snap" class="muted">up {{ span(now - snap.started_ms) }}</span>
    <span class="link" :class="link" role="status" aria-live="polite">
      <span class="dot" aria-hidden="true"></span>{{ LINK_TEXT[link] }}
    </span>
  </header>

  <main v-if="snap">
    <section v-if="snap.provider" aria-labelledby="h-provider">
      <h2 id="h-provider">Provider: {{ snap.provider.name }}</h2>
      <dl class="stats">
        <div><dt>Balance</dt><dd>{{ usd(snap.provider.balance_usd) }}</dd></div>
        <div><dt>Spend</dt><dd>{{ usd(snap.provider.spend_per_hour_usd) }}/h</dd></div>
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
              <th scope="col" class="n">Last call</th><th scope="col" class="n">In flight</th>
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
            </tr>
            <tr v-if="snap.provider.pods.length === 0"><td colspan="9" class="empty">No owned pods.</td></tr>
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
                <td><span class="pill" :class="f.state === 'streaming' ? 'ok' : 'warn'">{{ f.state }}</span></td>
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
                <th scope="col" class="n">Tokens in / out</th><th scope="col" class="n">Cache r / w</th><th scope="col" class="n">In flight</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="r in snap.routes" :key="r.route">
                <th scope="row">{{ r.route }}</th>
                <td class="n">{{ count(r.calls) }}</td>
                <td class="n" :class="{ 'bad-text': r.errors > 0 }">{{ count(r.errors) }}</td>
                <td class="n">{{ tokenPair(r.input_tokens, r.output_tokens) }}</td>
                <td class="n">{{ tokenPair(r.cache_read_tokens, r.cache_creation_tokens) }}</td>
                <td class="n">{{ r.in_flight }}</td>
              </tr>
              <tr v-if="snap.routes.length === 0"><td colspan="6" class="empty">No routes yet.</td></tr>
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
              <td class="n">{{ rate(s.input_tps) }}</td>
              <td class="n">{{ rate(s.output_tps) }}</td>
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
              <th scope="col">Flags</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="c in snap.calls" :key="c.id">
              <td class="mono" :title="ago(now, c.started_ms)">{{ clock(c.started_ms) }}</td>
              <td>{{ c.route ?? DASH }}<div v-if="c.upstream" class="muted small">{{ c.upstream }}</div></td>
              <td>{{ c.wire }}</td>
              <td class="n"><span class="pill" :class="statusClass(c)" :title="c.outcome">{{ c.status }}</span></td>
              <td class="n">{{ ms(c.ttft_ms) }}</td>
              <td class="n">{{ ms(c.duration_ms) }}</td>
              <td class="n">{{ ms(c.queue_wait_ms) }}</td>
              <td class="n">{{ tokenPair(c.input_tokens, c.output_tokens) }}</td>
              <td class="n">{{ tokenPair(c.cache_read_tokens, c.cache_creation_tokens) }}</td>
              <td><span v-for="f in flags(c)" :key="f" class="flag">{{ f }}</span></td>
            </tr>
            <tr v-if="snap.calls.length === 0"><td colspan="10" class="empty">No calls yet.</td></tr>
          </tbody>
        </table>
      </div>
    </section>
  </main>

  <main v-else class="waiting">
    <p>{{ feed.error.value ? `Cannot reach the gateway: ${feed.error.value}` : 'Waiting for the first snapshot…' }}</p>
  </main>
</template>
