<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue';
import { FlowModel, FONT_PX, NODE_R, type FlowCounts, type Scene } from './flow.ts';
import type { Snapshot } from './extra.ts';

// The ambient token-flow layer: drawn behind the page, never focusable, never a target.
const props = defineProps<{ snap: Snapshot | null }>();

const FRAME_MS = 1000 / 30;
const MAX_DPR = 2;
const REDUCED_QUERY = '(prefers-reduced-motion: reduce)';
const DARK_QUERY = '(prefers-color-scheme: dark)';

interface Colours { line: string; label: string; node: string; in: string; out: string }

const canvas = ref<HTMLCanvasElement | null>(null);
const media = (q: string) => (typeof matchMedia === 'function' ? matchMedia(q) : null);
const reducedQuery = media(REDUCED_QUERY);
const darkQuery = media(DARK_QUERY);
const model = new FlowModel({ reduced: reducedQuery?.matches ?? false, seed: 0x5eed });

let ctx: CanvasRenderingContext2D | null = null;
let width = 0;
let height = 0;
let raf = 0;
let lastFrame = 0;
let frames = 0;
let colours: Colours = { line: '#5b616b', label: '#5b616b', node: '#5b616b', in: '#2456c7', out: '#c2410c' };
let observer: MutationObserver | null = null;

function readColours() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  colours = { line: v('--muted', colours.line), label: v('--muted', colours.label), node: v('--muted', colours.node), in: v('--flow-in', colours.in), out: v('--flow-out', colours.out) };
}

function resize() {
  const el = canvas.value;
  if (!el) return;
  const dpr = Math.min(MAX_DPR, window.devicePixelRatio || 1);
  width = window.innerWidth;
  height = window.innerHeight;
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
    c.globalAlpha = s.alpha * (0.22 + (s.inFlight ? 0.12 + 0.25 * s.pulse : 0));
    c.strokeStyle = s.inFlight ? colours.in : colours.line;
    c.lineWidth = s.inFlight ? 1.5 + s.pulse : 1.25;
    c.beginPath();
    c.moveTo(s.from.x, s.from.y);
    c.bezierCurveTo(s.from.x + dx, s.from.y, s.to.x - dx, s.to.y, s.to.x, s.to.y);
    c.stroke();
  }
  c.font = `${FONT_PX}px ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace`;
  c.textBaseline = 'top';
  c.fillStyle = colours.node;
  for (const a of scene.sessions) {
    c.globalAlpha = a.alpha * 0.55;
    c.beginPath();
    c.arc(a.x, a.y, NODE_R - 1, 0, 2 * Math.PI);
    c.fill();
    c.globalAlpha = a.alpha * 0.75;
    c.fillText(a.text, a.box.x, a.box.y);
  }
  for (const r of scene.routes) {
    c.globalAlpha = 0.55;
    c.beginPath();
    c.arc(r.x, r.y, NODE_R, 0, 2 * Math.PI);
    c.fill();
    c.globalAlpha = 0.75;
    c.fillText(r.text, r.box.x, r.box.y);
  }
  for (const p of scene.particles) {
    c.globalAlpha = 0.6;
    c.fillStyle = p.dir === 'in' ? colours.in : colours.out;
    c.beginPath();
    c.arc(p.x, p.y, p.r, 0, 2 * Math.PI);
    c.fill();
  }
  c.globalAlpha = 1;
}

function draw() {
  if (!ctx || width === 0) return;
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
  return { strings: model.stringCount(), particles: model.particles.length, bursts: model.bursts, frames };
}

onMounted(() => {
  readColours();
  resize();
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
  window.removeEventListener('resize', resize);
  document.removeEventListener('visibilitychange', onVisibility);
  darkQuery?.removeEventListener?.('change', onTheme);
  reducedQuery?.removeEventListener?.('change', onReduced);
  observer?.disconnect();
  if (window.__flow === counts) delete window.__flow;
});
</script>

<template>
  <canvas ref="canvas" class="flow" aria-hidden="true"></canvas>
</template>
