export const DASH = '–';

export function count(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  const a = Math.abs(n);
  if (a < 1000) return String(Math.round(n));
  if (a < 1_000_000) return `${trim(n / 1000)}k`;
  if (a < 1_000_000_000) return `${trim(n / 1_000_000)}M`;
  return `${trim(n / 1_000_000_000)}G`;
}

function trim(x: number): string {
  const a = Math.abs(x);
  return a >= 100 ? x.toFixed(0) : a >= 10 ? x.toFixed(1) : x.toFixed(2);
}

export function rate(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  return n >= 100 ? n.toFixed(0) : n.toFixed(1);
}

export function ms(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  if (n < 1000) return `${Math.round(n)} ms`;
  if (n < 60_000) return `${(n / 1000).toFixed(n < 10_000 ? 2 : 1)} s`;
  return span(n);
}

export function span(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  const s = Math.max(0, Math.floor(n / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

export function ago(nowMs: number, thenMs: number | null | undefined): string {
  if (thenMs === null || thenMs === undefined) return DASH;
  return `${span(nowMs - thenMs)} ago`;
}

export function usd(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  return `$${n.toFixed(n < 10 ? 3 : 2)}`;
}

export function tps(n: number | null | undefined): string {
  if (n === null || n === undefined) return DASH;
  return `${count(n)} tok/s`;
}

export function pct(fraction: number | null | undefined): string {
  if (fraction === null || fraction === undefined || !Number.isFinite(fraction)) return DASH;
  const p = fraction * 100;
  return `${p >= 10 || p === 0 ? p.toFixed(0) : p.toFixed(1)}%`;
}

export function countdown(nowMs: number, atMs: number | null | undefined): string {
  if (atMs === null || atMs === undefined) return DASH;
  const left = atMs - nowMs;
  return left <= 0 ? 'now' : `in ${span(left)}`;
}

export function clock(t: number): string {
  const d = new Date(t);
  const p = (x: number) => String(x).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}
