<script setup lang="ts">
import { computed } from 'vue';
import { DASH, ago, count, countdown, pct, tps, usd } from './format.ts';
import type { LimitRow, Snapshot } from './extra.ts';

const props = defineProps<{ snap: Snapshot; now: number }>();

const UNIFIED = 'anthropic-ratelimit-unified-';
const IN_TILES = [
  'status', 'reset', 'representative-claim', 'overage-status', 'overage-disabled-reason',
  '5h-status', '5h-utilization', '5h-reset', '7d-status', '7d-utilization', '7d-reset',
].map((k) => UNIFIED + k);

const kpi = computed(() => props.snap.kpi);

interface Window { name: string; tone: string; utilization: string; reset: number | null }
interface Limit {
  row: LimitRow;
  status: string | null;
  tone: string;
  reset: number | null;
  claim: string | null;
  overage: string | null;
  overageTone: string;
  overageReason: string | null;
  windows: Window[];
  rest: [string, string][];
}

function tone(status: string | null | undefined): string {
  if (status === 'allowed') return 'ok';
  if (status === 'allowed_warning') return 'warn';
  if (status === 'rejected') return 'bad';
  return '';
}

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

const limits = computed<Limit[]>(() =>
  (props.snap.limits ?? []).map((row) => {
    const h: Record<string, string> = {};
    for (const [k, v] of Object.entries(row.headers)) h[k.toLowerCase()] = v;
    const u = (k: string) => h[UNIFIED + k];
    const windows: Window[] = (['5h', '7d'] as const)
      .filter((w) => u(`${w}-utilization`) !== undefined || u(`${w}-status`) !== undefined)
      .map((w) => ({
        name: w,
        tone: tone(u(`${w}-status`)),
        utilization: pct(fraction(u(`${w}-utilization`))),
        reset: unixMs(u(`${w}-reset`)),
      }));
    const rest = Object.entries(h)
      .filter(([k]) => !IN_TILES.includes(k))
      .sort(([a], [b]) => a.localeCompare(b));
    return {
      row,
      status: u('status') ?? null,
      tone: tone(u('status')),
      reset: unixMs(u('reset')),
      claim: u('representative-claim') ?? null,
      overage: u('overage-status') ?? null,
      overageTone: tone(u('overage-status')),
      overageReason: u('overage-disabled-reason') ?? null,
      windows,
      rest,
    };
  }),
);
</script>

<template>
  <section aria-labelledby="h-kpi">
    <h2 id="h-kpi">Overview</h2>
    <div class="tiles">
      <div v-if="kpi" class="tile">
        <div class="label">Tokens in</div>
        <div class="value">{{ count(kpi.input_tokens) }}</div>
        <div class="sub">{{ tps(kpi.input_rate) }} <span class="muted">last 60 s</span></div>
      </div>
      <div v-if="kpi" class="tile">
        <div class="label">Tokens out</div>
        <div class="value">{{ count(kpi.output_tokens) }}</div>
        <div class="sub">{{ tps(kpi.output_rate) }} <span class="muted">last 60 s</span></div>
      </div>
      <div v-if="kpi" class="tile">
        <div class="label"><abbr title="cache read / every prompt token, over every route: Anthropic's reported cache, and a vLLM server's prefix-cache hits over its queries">Cache hit rate</abbr></div>
        <div class="value">{{ pct(kpi.cache_hit_rate) }}</div>
        <div class="sub">
          read {{ count(kpi.cache_read_tokens) }} · created {{ count(kpi.cache_creation_tokens) }} · uncached {{ count(kpi.uncached_input_tokens) }}
        </div>
        <table v-if="kpi.cache_routes.length > 0" class="mini">
          <tbody>
            <tr v-for="r in kpi.cache_routes" :key="r.route">
              <th scope="row">{{ r.route }} <span v-if="r.source === 'vllm'" class="badge">vLLM</span></th>
              <td class="n">{{ pct(r.hit_rate) }}</td>
              <td class="n">{{ count(r.read_tokens) }} / {{ count(r.prompt_tokens) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-if="kpi" class="tile wide">
        <div class="label">Throughput, last 60 s</div>
        <table class="mini">
          <tbody>
            <tr v-for="r in kpi.route_rates" :key="r.route">
              <th scope="row">{{ r.route }}</th>
              <td class="n">in {{ tps(r.input_tps) }}</td>
              <td class="n">out {{ tps(r.output_tps) }}</td>
            </tr>
            <tr v-if="kpi.route_rates.length === 0"><td class="muted">Idle.</td></tr>
          </tbody>
        </table>
      </div>
      <div v-if="snap.provider" class="tile">
        <div class="label">{{ snap.provider.name }} balance</div>
        <div class="value">{{ usd(snap.provider.balance_usd) }}</div>
        <div class="sub">{{ usd(snap.provider.spend_per_hour_usd) }}/h spend</div>
      </div>
    </div>

    <div v-for="l in limits" :key="l.row.credential" class="limit">
      <h3>
        Subscription limits <span class="mono">{{ l.row.credential }}</span>
        <span class="muted small">seen {{ ago(now, l.row.seen_ms) }}</span>
      </h3>
      <div class="tiles">
        <div class="tile" :class="l.tone">
          <div class="label">Status</div>
          <div class="value">{{ l.status ?? DASH }}</div>
          <div v-if="l.reset !== null" class="sub">resets {{ countdown(now, l.reset) }}</div>
        </div>
        <div v-for="w in l.windows" :key="w.name" class="tile" :class="w.tone">
          <div class="label">{{ w.name }} utilization</div>
          <div class="value">{{ w.utilization }}</div>
          <div class="sub">resets {{ countdown(now, w.reset) }}</div>
        </div>
        <div v-if="l.overage !== null" class="tile" :class="l.overageTone">
          <div class="label">Overage</div>
          <div class="value">{{ l.overage }}</div>
          <div v-if="l.overageReason" class="sub">{{ l.overageReason }}</div>
        </div>
        <div v-if="l.claim !== null" class="tile">
          <div class="label"><abbr title="the window that currently decides the status">Representative claim</abbr></div>
          <div class="value">{{ l.claim }}</div>
        </div>
      </div>
      <dl v-if="l.rest.length" class="headers">
        <template v-for="[k, v] in l.rest" :key="k">
          <dt class="mono">{{ k }}</dt>
          <dd class="mono">{{ v }}</dd>
        </template>
      </dl>
    </div>
  </section>
</template>
