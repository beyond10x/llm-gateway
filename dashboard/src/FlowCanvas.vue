<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { FLIGHT_PULSE, FlowModel, FONT_PX, INK, NODE_R, activeSessions, type FlowCounts, type Mark, type Scene } from './flow.ts';
import type { Snapshot } from './extra.ts';

// The token flow panel: a canvas the width of the page that draws each active session's strings to
// the routes it calls. The canvas is decoration (aria-hidden, never focusable, never a target); the
// caption above it says in words what it shows.
const props = defineProps<{ snap: Snapshot | null }>();

const FRAME_MS = 1000 / 30;
const MAX_DPR = 2;
const REDUCED_QUERY = '(prefers-reduced-motion: reduce)';
const DARK_QUERY = '(prefers-color-scheme: dark)';

type Colours = Record<Mark, string>;

const canvas = ref<HTMLCanvasElement | null>(null);
const box = ref<HTMLDivElement | null>(null);
const media = (q: string) => (typeof matchMedia === 'function' ? matchMedia(q) : null);
const reducedQuery = media(REDUCED_QUERY);
const darkQuery = media(DARK_QUERY);
const model = new FlowModel({ reduced: reducedQuery?.matches ?? false, seed: 0x5eed });

const summary = computed(() => {
  if (!props.snap) return { sessions: 0, routes: 0 };
  const active = activeSessions(props.snap);
  const routes = new Set<string>();
  for (const a of active.values()) for (const r of a.routes.keys()) routes.add(r);
  return { sessions: active.size, routes: routes.size };
});

function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? '' : 's'}`;
}

let ctx: CanvasRenderingContext2D | null = null;
let width = 0;
let height = 0;
let raf = 0;
let lastFrame = 0;
let frames = 0;
let drawn = 0;
let colours: Colours = { label: '#1b1d21', note: '#1b1d21', idleString: '#5b616b', flightString: '#2456c7', node: '#5b616b', inDot: '#2456c7', outDot: '#c2410c' };
let observer: MutationObserver | null = null;
let sizer: ResizeObserver | null = null;

function readColours() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  const next = { ...colours };
  for (const mark of Object.keys(INK) as Mark[]) next[mark] = v(INK[mark].colour, colours[mark]);
  colours = next;
}

function resize() {
  const el = canvas.value;
  const b = box.value;
  if (!el || !b) return;
  if (ctx && b.clientWidth === width && b.clientHeight === height) return;
  const dpr = Math.min(MAX_DPR, window.devicePixelRatio || 1);
  width = b.clientWidth;
  height = b.clientHeight;
  el.width = Math.round(width * dpr);
  el.height = Math.round(height * dpr);
  ctx = el.getContext('2d');
  ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
  draw();
}

function paint(c: CanvasRenderingContext2D, scene: Scene) {
  c.clearRect(0, 0, width, height);
  c.lineCap = 'round';
  for (const s of scene.strings) {
    const dx = (s.to.x - s.from.x) * 0.45;
    c.globalAlpha = s.alpha * (s.inFlight ? INK.flightString.alpha + FLIGHT_PULSE * s.pulse : INK.idleString.alpha);
    c.strokeStyle = s.inFlight ? colours.flightString : colours.idleString;
    c.lineWidth = s.inFlight ? 1.5 + s.pulse : 1.25;
    c.beginPath();
    c.moveTo(s.from.x, s.from.y);
    c.bezierCurveTo(s.from.x + dx, s.from.y, s.to.x - dx, s.to.y, s.to.x, s.to.y);
    c.stroke();
  }
  c.font = `${FONT_PX}px ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace`;
  c.textBaseline = 'middle';
  const node = (x: number, y: number, r: number, alpha: number) => {
    c.fillStyle = colours.node;
    c.globalAlpha = alpha * INK.node.alpha;
    c.beginPath();
    c.arc(x, y, r, 0, 2 * Math.PI);
    c.fill();
  };
  const label = (text: string, x: number, y: number, alpha: number, mark: 'label' | 'note' = 'label') => {
    c.fillStyle = colours[mark];
    c.globalAlpha = alpha * INK[mark].alpha;
    c.fillText(text, x, y);
  };
  for (const a of scene.sessions) {
    node(a.x, a.y, NODE_R - 1, a.alpha);
    label(a.text, a.box.x, a.y, a.alpha);
  }
  for (const r of scene.routes) {
    node(r.x, r.y, NODE_R, 1);
    label(r.text, r.box.x, r.y, 1);
  }
  for (const n of scene.notes) label(n.text, n.box.x, n.box.y + n.box.h / 2, 1, 'note');
  for (const p of scene.particles) {
    const mark = p.dir === 'in' ? 'inDot' : 'outDot';
    c.globalAlpha = INK[mark].alpha;
    c.fillStyle = colours[mark];
    c.beginPath();
    c.arc(p.x, p.y, p.r, 0, 2 * Math.PI);
    c.fill();
  }
  drawn += scene.particles.length;
  c.globalAlpha = 1;
}

function draw() {
  if (!ctx || width === 0 || height === 0) return;
  paint(ctx, model.scene(width, height));
  frames++;
}

function loop(ts: number) {
  raf = requestAnimationFrame(loop);
  if (ts - lastFrame < FRAME_MS - 1) return;
  const dt = Math.min(0.1, lastFrame === 0 ? 0 : (ts - lastFrame) / 1000);
  lastFrame = ts;
  model.step(dt);
  if (model.takeDirty()) draw();
}

function start() {
  if (raf !== 0 || model.reduced || document.hidden) return;
  lastFrame = 0;
  raf = requestAnimationFrame(loop);
}

function stop() {
  if (raf !== 0) cancelAnimationFrame(raf);
  raf = 0;
}

function onVisibility() {
  if (document.hidden) stop();
  else start();
}

function onTheme() {
  readColours();
  draw();
}

function onReduced() {
  model.reduced = reducedQuery?.matches ?? false;
  if (model.reduced) {
    stop();
    model.particles = [];
    draw();
  } else start();
}

watch(
  () => props.snap,
  (s) => {
    if (!s) return;
    model.apply(s);
    if (model.reduced) draw();
  },
  { immediate: true },
);

function counts(): FlowCounts {
  return { strings: model.stringCount(), particles: model.particles.length, drawn, bursts: model.bursts, frames };
}

onMounted(() => {
  readColours();
  resize();
  if (typeof ResizeObserver === 'function' && box.value) {
    sizer = new ResizeObserver(() => resize());
    sizer.observe(box.value);
  }
  window.addEventListener('resize', resize);
  document.addEventListener('visibilitychange', onVisibility);
  darkQuery?.addEventListener?.('change', onTheme);
  reducedQuery?.addEventListener?.('change', onReduced);
  observer = new MutationObserver(onTheme);
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
  if (import.meta.env.MODE === 'test' || new URLSearchParams(location.search).has('flowdebug')) window.__flow = counts;
  start();
});

onUnmounted(() => {
  stop();
  sizer?.disconnect();
  window.removeEventListener('resize', resize);
  document.removeEventListener('visibilitychange', onVisibility);
  darkQuery?.removeEventListener?.('change', onTheme);
  reducedQuery?.removeEventListener?.('change', onReduced);
  observer?.disconnect();
  if (window.__flow === counts) delete window.__flow;
});
</script>

<template>
  <section class="flow-panel" aria-labelledby="h-flow">
    <div class="flow-head">
      <h2 id="h-flow">Token flow</h2>
      <p class="flow-count">{{ summary.sessions > 0 ? `${plural(summary.sessions, 'active session')} → ${plural(summary.routes, 'route')}` : 'no active session' }}</p>
    </div>
    <p class="flow-caption">
      <span class="flow-key"><span class="swatch in" aria-hidden="true"></span>input tokens</span>
      <span class="flow-key"><span class="swatch out" aria-hidden="true"></span>output tokens</span>
    </p>
    <div ref="box" class="flow-box">
      <canvas ref="canvas" aria-hidden="true"></canvas>
      <p v-if="summary.sessions === 0" class="flow-empty">No active sessions.</p>
    </div>
  </section>
</template>
