// The token-flow layer behind the dashboard: one string per active session and route, labelled
// with the session on the left and the route on the right, with particles for each new call's
// tokens: input from the session to the route, output back. Everything here is pure: the snapshot
// diff, the layout, and the model's step take the clock and the random source from the caller.
import type { Snapshot } from './extra.ts';

export const ACTIVE_MS = 5 * 60_000;
export const SEED_BURSTS = 3;
export const MAX_PARTICLES_PER_BURST = 12;
export const MAX_PARTICLES = 400;
export const MAX_SESSIONS = 12;
export const MAX_ROUTES = 10;
export const FADE_PER_S = 0.8;
export const PULSE_MS = 2400;

export interface Burst {
  call: number;
  session: string;
  route: string;
  input: number | null;
  output: number | null;
}

export interface Diff {
  known: Set<number>;
  bursts: Burst[];
}

function burstOf(c: Snapshot['calls'][number]): Burst | null {
  if (c.route === null) return null;
  return { call: c.id, session: c.session, route: c.route, input: c.input_tokens, output: c.output_tokens };
}

// The calls of `snap` not in `known`. The first snapshot (`known` null) only seeds the ids, and
// replays at most SEED_BURSTS of its newest calls that started within ACTIVE_MS.
export function diff(known: ReadonlySet<number> | null, snap: Snapshot): Diff {
  const ids = new Set(snap.calls.map((c) => c.id));
  const bursts: Burst[] = [];
  if (known === null) {
    const recent = [...snap.calls]
      .filter((c) => snap.now_ms - c.started_ms <= ACTIVE_MS)
      .sort((a, b) => b.id - a.id);
    for (const c of recent) {
      if (bursts.length >= SEED_BURSTS) break;
      const b = burstOf(c);
      if (b) bursts.push(b);
    }
  } else {
    for (const c of [...snap.calls].sort((a, b) => a.id - b.id)) {
      if (known.has(c.id)) continue;
      const b = burstOf(c);
      if (b) bursts.push(b);
    }
  }
  return { known: ids, bursts };
}

export interface ActiveSession {
  session: string;
  label: string;
  routes: Map<string, number>;
}

// The sessions that are active at the snapshot's time: an in-flight call, or last seen within
// ACTIVE_MS. Each carries the routes it reached within ACTIVE_MS (or its newest call's route), with
// the number of its in-flight calls on each.
export function activeSessions(snap: Snapshot): Map<string, ActiveSession> {
  const out = new Map<string, ActiveSession>();
  const labels = new Map(snap.sessions.map((s) => [s.session, s.label]));
  const add = (session: string): ActiveSession => {
    let a = out.get(session);
    if (!a) {
      a = { session, label: labels.get(session) ?? '', routes: new Map() };
      out.set(session, a);
    }
    return a;
  };
  for (const s of snap.sessions) {
    if (snap.now_ms - s.last_ms <= ACTIVE_MS) add(s.session);
  }
  for (const f of snap.in_flight) {
    const a = add(f.session);
    if (f.route !== null) a.routes.set(f.route, (a.routes.get(f.route) ?? 0) + 1);
  }
  const newest = new Map<string, string>();
  for (const c of [...snap.calls].sort((x, y) => y.id - x.id)) {
    if (c.route === null) continue;
    if (!newest.has(c.session)) newest.set(c.session, c.route);
    const a = out.get(c.session);
    if (a && snap.now_ms - c.started_ms <= ACTIVE_MS && !a.routes.has(c.route)) a.routes.set(c.route, 0);
  }
  for (const a of out.values()) {
    const r = newest.get(a.session);
    if (a.routes.size === 0 && r !== undefined) a.routes.set(r, 0);
  }
  return out;
}

export function sessionLabel(session: string, label: string): string {
  const id = session.length > 8 ? `${session.slice(0, 8)}…` : session;
  return label ? `${id} · ${label}` : id;
}

// Particles for `tokens`: a log scale, capped, and none for no tokens.
export function particleCount(tokens: number | null): number {
  if (tokens === null || tokens <= 0) return 0;
  return Math.min(MAX_PARTICLES_PER_BURST, 1 + Math.floor(Math.log10(tokens) * 2));
}

export function particleRadius(tokens: number | null): number {
  if (tokens === null || tokens <= 0) return 1.2;
  return Math.min(3.5, 1.2 + Math.log10(tokens) * 0.4);
}

// A seeded random source (mulberry32), so a test sees the same particles every run.
export function seeded(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---------------------------------------------------------------------------------------------
// Layout

export const FONT_PX = 11;
export const CHAR_PX = 6.7;
export const PAD = 12;
export const TOP = 64;
export const BOTTOM = 24;
export const NODE_R = 4;

export interface Point { x: number; y: number }
export interface Box { x: number; y: number; w: number; h: number }
export interface Anchor extends Point { key: string; text: string; box: Box }
export interface Layout { width: number; height: number; sessions: Map<string, Anchor>; routes: Map<string, Anchor> }

function fit(text: string, maxPx: number): string {
  const max = Math.max(1, Math.floor(maxPx / CHAR_PX));
  return text.length <= max ? text : `${text.slice(0, Math.max(0, max - 1))}…`;
}

function rows(n: number, height: number): number[] {
  const top = Math.min(TOP, height * 0.2);
  const span = Math.max(0, height - top - BOTTOM);
  return Array.from({ length: n }, (_, i) => top + ((i + 0.5) * span) / n);
}

// Session anchors on the left, route nodes on the right, each spread evenly down the page. A label
// sits above its anchor and is cut to fit inside the canvas.
export function layout(sessions: { key: string; text: string }[], routes: string[], width: number, height: number): Layout {
  const maxLabel = Math.max(CHAR_PX * 4, width * 0.45 - PAD);
  const sy = rows(sessions.length, height);
  const ry = rows(routes.length, height);
  const out: Layout = { width, height, sessions: new Map(), routes: new Map() };
  sessions.forEach((s, i) => {
    const text = fit(s.text, maxLabel);
    const x = PAD + NODE_R;
    const y = sy[i]!;
    out.sessions.set(s.key, { key: s.key, text, x, y, box: { x: PAD, y: y - NODE_R - 3 - FONT_PX, w: text.length * CHAR_PX, h: FONT_PX } });
  });
  routes.forEach((r, i) => {
    const text = fit(r, maxLabel);
    const x = width - PAD - NODE_R;
    const y = ry[i]!;
    const w = text.length * CHAR_PX;
    out.routes.set(r, { key: r, text, x, y, box: { x: width - PAD - w, y: y - NODE_R - 3 - FONT_PX, w, h: FONT_PX } });
  });
  return out;
}

// A point on the string from `a` to `b` at `t` in [0, 1]: a cubic curve that leaves and arrives
// level.
export function curve(a: Point, b: Point, t: number): Point {
  const dx = (b.x - a.x) * 0.45;
  const c1 = { x: a.x + dx, y: a.y };
  const c2 = { x: b.x - dx, y: b.y };
  const u = 1 - t;
  return {
    x: u * u * u * a.x + 3 * u * u * t * c1.x + 3 * u * t * t * c2.x + t * t * t * b.x,
    y: u * u * u * a.y + 3 * u * u * t * c1.y + 3 * u * t * t * c2.y + t * t * t * b.y,
  };
}

// ---------------------------------------------------------------------------------------------
// Model

export interface Particle {
  session: string;
  route: string;
  dir: 'in' | 'out';
  t: number;
  delay: number;
  speed: number;
  r: number;
}

interface Lane { label: string; alpha: number; target: number; strings: Map<string, Strand> }
interface Strand { alpha: number; target: number; inFlight: number }

export interface Scene {
  strings: { from: Point; to: Point; alpha: number; inFlight: boolean; pulse: number }[];
  sessions: (Anchor & { alpha: number })[];
  routes: Anchor[];
  particles: (Point & { r: number; dir: 'in' | 'out' })[];
}

export interface FlowCounts { strings: number; particles: number; bursts: number; frames: number }

export class FlowModel {
  readonly lanes = new Map<string, Lane>();
  routes: string[] = [];
  particles: Particle[] = [];
  bursts = 0;
  time = 0;
  private known: Set<number> | null = null;
  private dirty = true;
  reduced: boolean;
  private readonly random: () => number;

  constructor(options: { reduced?: boolean; seed?: number } = {}) {
    this.reduced = options.reduced ?? false;
    this.random = seeded(options.seed ?? 1);
  }

  // Takes a snapshot: marks sessions and strings active or ending, and starts a burst for each new
  // call. Returns the bursts.
  apply(snap: Snapshot): Burst[] {
    const d = diff(this.known, snap);
    this.known = d.known;
    const active = activeSessions(snap);
    for (const b of d.bursts) {
      const a = active.get(b.session) ?? { session: b.session, label: snap.sessions.find((s) => s.session === b.session)?.label ?? '', routes: new Map<string, number>() };
      if (!a.routes.has(b.route)) a.routes.set(b.route, 0);
      active.set(b.session, a);
    }
    for (const [key, lane] of this.lanes) {
      const a = active.get(key);
      lane.target = a ? 1 : 0;
      for (const [route, s] of lane.strings) {
        s.target = a?.routes.has(route) ? 1 : 0;
        s.inFlight = a?.routes.get(route) ?? 0;
      }
    }
    for (const a of [...active.values()].slice(0, MAX_SESSIONS)) {
      let lane = this.lanes.get(a.session);
      if (!lane) {
        if (this.lanes.size >= MAX_SESSIONS) continue;
        lane = { label: a.label, alpha: 0, target: 1, strings: new Map() };
        this.lanes.set(a.session, lane);
      }
      lane.label = a.label || lane.label;
      for (const [route, inFlight] of a.routes) {
        const s = lane.strings.get(route);
        if (s) continue;
        lane.strings.set(route, { alpha: 0, target: 1, inFlight });
      }
    }
    const order = snap.routes.map((r) => r.route);
    for (const lane of this.lanes.values()) for (const route of lane.strings.keys()) if (!order.includes(route)) order.push(route);
    this.routes = order.slice(0, MAX_ROUTES);
    for (const b of d.bursts) this.burst(b);
    if (this.reduced) this.settle();
    this.dirty = true;
    return d.bursts;
  }

  private burst(b: Burst) {
    this.bursts++;
    if (this.reduced || !this.lanes.get(b.session)?.strings.has(b.route) || !this.routes.includes(b.route)) return;
    const input = particleCount(b.input) || (b.output === null ? 1 : 0);
    const output = particleCount(b.output);
    for (let i = 0; i < input; i++) this.spawn(b, 'in', i * 0.08, particleRadius(b.input));
    for (let i = 0; i < output; i++) this.spawn(b, 'out', 0.9 + i * 0.08, particleRadius(b.output));
  }

  private spawn(b: Burst, dir: 'in' | 'out', delay: number, r: number) {
    if (this.particles.length >= MAX_PARTICLES) return;
    this.particles.push({ session: b.session, route: b.route, dir, t: 0, delay: delay + this.random() * 0.1, speed: 0.45 + this.random() * 0.3, r });
  }

  // Fades finish at once, and ended strings and sessions go.
  private settle() {
    for (const lane of this.lanes.values()) {
      lane.alpha = lane.target;
      for (const s of lane.strings.values()) s.alpha = s.target;
    }
    this.prune();
  }

  private prune() {
    for (const [key, lane] of this.lanes) {
      for (const [route, s] of lane.strings) if (s.target === 0 && s.alpha <= 0) lane.strings.delete(route);
      if (lane.target === 0 && lane.alpha <= 0) this.lanes.delete(key);
    }
    this.particles = this.particles.filter((p) => this.lanes.get(p.session)?.strings.has(p.route) && this.routes.includes(p.route));
  }

  // Advances `dt` seconds. Under reduced motion nothing moves.
  step(dt: number) {
    if (this.reduced) return;
    this.time += dt * 1000;
    const fade = (from: number, to: number) => (from < to ? Math.min(to, from + FADE_PER_S * dt) : Math.max(to, from - FADE_PER_S * dt));
    let changed = this.particles.length > 0;
    for (const lane of this.lanes.values()) {
      if (lane.alpha !== lane.target) changed = true;
      lane.alpha = fade(lane.alpha, lane.target);
      for (const s of lane.strings.values()) {
        if (s.alpha !== s.target || s.inFlight > 0) changed = true;
        s.alpha = fade(s.alpha, s.target);
      }
    }
    for (const p of this.particles) {
      if (p.delay > 0) p.delay -= dt;
      else p.t += p.speed * dt;
    }
    this.particles = this.particles.filter((p) => p.t < 1);
    this.prune();
    if (changed) this.dirty = true;
  }

  // Whether the next frame differs from the last one drawn.
  takeDirty(): boolean {
    const d = this.dirty;
    this.dirty = false;
    return d;
  }

  stringCount(): number {
    let n = 0;
    for (const lane of this.lanes.values()) for (const route of lane.strings.keys()) if (this.routes.includes(route)) n++;
    return n;
  }

  layout(width: number, height: number): Layout {
    const sessions = [...this.lanes].map(([key, lane]) => ({ key, text: sessionLabel(key, lane.label) }));
    return layout(sessions, this.routes, width, height);
  }

  scene(width: number, height: number): Scene {
    const l = this.layout(width, height);
    const pulse = this.reduced ? 0 : 0.5 - 0.5 * Math.cos((2 * Math.PI * this.time) / PULSE_MS);
    const scene: Scene = { strings: [], sessions: [], routes: [...l.routes.values()], particles: [] };
    for (const [key, lane] of this.lanes) {
      const from = l.sessions.get(key)!;
      scene.sessions.push({ ...from, alpha: lane.alpha });
      for (const [route, s] of lane.strings) {
        const to = l.routes.get(route);
        if (!to) continue;
        scene.strings.push({ from, to, alpha: Math.min(lane.alpha, s.alpha), inFlight: s.inFlight > 0, pulse: s.inFlight > 0 ? pulse : 0 });
      }
    }
    for (const p of this.particles) {
      if (p.delay > 0) continue;
      const a = l.sessions.get(p.session);
      const b = l.routes.get(p.route);
      if (!a || !b) continue;
      const at = p.dir === 'in' ? curve(a, b, p.t) : curve(b, a, p.t);
      scene.particles.push({ ...at, r: p.r, dir: p.dir });
    }
    return scene;
  }
}

declare global {
  interface Window {
    __flow?: () => FlowCounts;
  }
}
