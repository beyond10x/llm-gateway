import { describe, expect, test } from 'vitest';
import type { CallRecord, Snapshot } from './extra.ts';
import { fixture } from './fixture.ts';
import { FlowModel, SEED_BURSTS, diff, layout, particleCount, sessionLabel } from './flow.ts';

const NOW = 1_800_000_000_000;

function call(id: number, session: string, route: string | null, input: number | null, output: number | null, startedMs: number): CallRecord {
  return { ...fixture(NOW).calls[0]!, id, session, route, input_tokens: input, output_tokens: output, started_ms: startedMs };
}

function snap(now: number, parts: Partial<Snapshot>): Snapshot {
  return { ...fixture(now), calls: [], in_flight: [], sessions: [], ...parts };
}

function session(id: string, label: string, lastMs: number): Snapshot['sessions'][number] {
  return { ...fixture(NOW).sessions[0]!, session: id, label, last_ms: lastMs };
}

describe('diff', () => {
  test('the first snapshot seeds every id and replays at most a few of its newest calls', () => {
    const s = fixture(NOW);
    const d = diff(null, s);
    expect(d.known.size).toBe(s.calls.length);
    expect(d.bursts.length).toBeGreaterThan(0);
    expect(d.bursts.length).toBeLessThanOrEqual(SEED_BURSTS);
    for (const b of d.bursts) expect(s.calls.find((c) => c.id === b.call)!.started_ms).toBeGreaterThan(NOW - 5 * 60_000);
  });

  test('a call id not seen before is one burst with its session, route and tokens', () => {
    const first = diff(null, fixture(NOW, 0));
    const next = fixture(NOW + 1000, 1);
    const d = diff(first.known, next);
    const fresh = next.calls.find((c) => !first.known.has(c.id))!;
    expect(d.bursts).toEqual([{ call: fresh.id, session: fresh.session, route: fresh.route, input: fresh.input_tokens, output: fresh.output_tokens }]);
    expect(diff(d.known, next).bursts).toEqual([]);
  });

  test('a refused call with no route makes no burst', () => {
    const known = new Set<number>();
    const d = diff(known, snap(NOW, { calls: [call(7, 'aa11bb22', null, null, null, NOW)] }));
    expect(d.bursts).toEqual([]);
    expect(d.known.has(7)).toBe(true);
  });
});

describe('scale', () => {
  test('particles grow on a log scale, are capped, and are none for no tokens', () => {
    expect(particleCount(null)).toBe(0);
    expect(particleCount(0)).toBe(0);
    expect(particleCount(1)).toBe(1);
    expect(particleCount(40)).toBe(4);
    expect(particleCount(2_000)).toBe(7);
    expect(particleCount(90_000)).toBe(10);
    expect(particleCount(1e9)).toBe(12);
  });

  test('a burst starts input particles and, after them, output particles', () => {
    const m = new FlowModel({ seed: 3 });
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)] }));
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)], calls: [call(1, 'aa11bb22', 'qwen-coder', 2_000, 40, NOW)] }));
    expect(m.bursts).toBe(1);
    expect(m.particles.filter((p) => p.dir === 'in')).toHaveLength(7);
    expect(m.particles.filter((p) => p.dir === 'out')).toHaveLength(4);
    expect(m.particles.every((p) => p.session === 'aa11bb22' && p.route === 'qwen-coder')).toBe(true);
  });
});

describe('model', () => {
  test('a session that ends fades its string out, and a new one fades in', () => {
    const m = new FlowModel({ seed: 1 });
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)], calls: [call(1, 'aa11bb22', 'qwen-coder', 10, 10, NOW)] }));
    expect(m.stringCount()).toBe(1);
    m.step(2);
    expect(m.lanes.get('aa11bb22')!.alpha).toBe(1);

    const later = NOW + 6 * 60_000;
    m.apply(snap(later, {
      sessions: [session('aa11bb22', 'codex', NOW), session('cc33dd44', 'claude-code', later)],
      calls: [call(1, 'aa11bb22', 'qwen-coder', 10, 10, NOW), call(2, 'cc33dd44', 'claude-sonnet', 10, 10, later)],
    }));
    m.step(0.5);
    const ending = m.lanes.get('aa11bb22')!;
    const starting = m.lanes.get('cc33dd44')!;
    expect(ending.alpha).toBeGreaterThan(0);
    expect(ending.alpha).toBeLessThan(1);
    expect(starting.alpha).toBeGreaterThan(0);
    expect(starting.alpha).toBeLessThan(1);
    m.step(1);
    m.step(1);
    expect(m.lanes.has('aa11bb22')).toBe(false);
    expect(m.lanes.get('cc33dd44')!.alpha).toBe(1);
    expect(m.stringCount()).toBe(1);
  });

  test('an in-flight call keeps its session active and pulses its string', () => {
    const m = new FlowModel({ seed: 1 });
    const inFlight = { ...fixture(NOW).in_flight[0]!, session: 'aa11bb22', route: 'qwen-coder' };
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW - 60 * 60_000)], in_flight: [inFlight] }));
    m.step(0.6);
    const a = m.scene(1280, 720).strings[0]!;
    m.step(0.6);
    const b = m.scene(1280, 720).strings[0]!;
    expect(a.inFlight).toBe(true);
    expect(a.pulse).not.toBe(b.pulse);
  });

  test('under reduced motion a frame is static: no particle moves and nothing pulses', () => {
    const m = new FlowModel({ reduced: true, seed: 1 });
    const inFlight = { ...fixture(NOW).in_flight[0]!, session: 'aa11bb22', route: 'qwen-coder' };
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)], in_flight: [inFlight] }));
    m.apply(snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)], in_flight: [inFlight], calls: [call(1, 'aa11bb22', 'qwen-coder', 90_000, 3_000, NOW)] }));
    const before = JSON.stringify(m.scene(1280, 720));
    m.step(0.5);
    m.step(0.5);
    const after = m.scene(1280, 720);
    expect(JSON.stringify(after)).toBe(before);
    expect(m.bursts).toBe(1);
    expect(after.particles).toEqual([]);
    expect(after.strings).toHaveLength(1);
    expect(after.strings[0]!.alpha).toBe(1);
    expect(after.strings[0]!.pulse).toBe(0);
  });

  test('particles move along the string when motion is allowed', () => {
    const m = new FlowModel({ seed: 1 });
    const s = snap(NOW, { sessions: [session('aa11bb22', 'codex', NOW)] });
    m.apply(s);
    m.apply({ ...s, calls: [call(1, 'aa11bb22', 'qwen-coder', 2_000, null, NOW)] });
    m.step(0.3);
    const a = m.scene(1280, 720).particles.map((p) => p.x);
    m.step(0.1);
    const b = m.scene(1280, 720).particles.map((p) => p.x);
    expect(a.length).toBeGreaterThan(0);
    expect(b.slice(0, a.length)).not.toEqual(a);
  });
});

describe('layout', () => {
  const sessions = Array.from({ length: 12 }, (_, i) => {
    const id = (i * 0x1f3a5c7 + 0xabcdef).toString(16).padStart(24, '0');
    return { key: id, text: sessionLabel(id, 'claude-code/2.1.97 (external, cli)') };
  });
  const routes = Array.from({ length: 10 }, (_, i) => `route-${i}-with-a-long-model-name-and-suffix`);

  for (const [w, h] of [[1280, 720], [390, 844]] as const) {
    test(`every label of 12 sessions and 10 routes lies inside a ${w}x${h} canvas`, () => {
      const l = layout(sessions, routes, w, h);
      expect(l.sessions.size).toBe(12);
      expect(l.routes.size).toBe(10);
      for (const a of [...l.sessions.values(), ...l.routes.values()]) {
        expect(a.box.x).toBeGreaterThanOrEqual(0);
        expect(a.box.y).toBeGreaterThanOrEqual(0);
        expect(a.box.x + a.box.w).toBeLessThanOrEqual(w);
        expect(a.box.y + a.box.h).toBeLessThanOrEqual(h);
        expect(a.x).toBeGreaterThan(0);
        expect(a.x).toBeLessThan(w);
      }
      const left = [...l.sessions.values()].map((a) => a.box.x + a.box.w);
      const right = [...l.routes.values()].map((a) => a.box.x);
      expect(Math.max(...left)).toBeLessThan(Math.min(...right));
    });
  }

  test('a session label is the first 8 characters of its id, then its label', () => {
    expect(sessionLabel('3f9a1c0e', 'claude-code')).toBe('3f9a1c0e · claude-code');
    expect(sessionLabel('3f9a1c0e5d6b7a8c9d0e1f2a', 'codex')).toBe('3f9a1c0e… · codex');
    expect(sessionLabel('3f9a1c0e', '')).toBe('3f9a1c0e');
  });
});

describe('fixture replay', () => {
  test('each tick adds one new call with a synthetic id', () => {
    const a = fixture(NOW, 4);
    const b = fixture(NOW + 1000, 5);
    const ids = new Set(a.calls.map((c) => c.id));
    const fresh = b.calls.filter((c) => !ids.has(c.id));
    expect(fresh).toHaveLength(1);
    expect(fresh[0]!.session).toMatch(/^[0-9a-f]{8}$/);
  });
});
