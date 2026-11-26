// The binding against the document: every placeholder it reads has an answer, every answer holds
// the fields its columns, fields and metrics read, and the fixture files under ui/fixtures/ are
// what the binding answers for the last state of the `errors` scenario. `UPDATE_UI_FIXTURES=1`
// writes them.
import { expect, test } from 'vitest';
import YAML from '../../ui/llm-gateway.ui.yaml?raw';
import { SCENARIOS } from '../../ux/scenarios/index.ts';
import type { Snapshot } from '../extra.ts';
import { BINDINGS, answer, failedCall, failureCause, recentByRoute } from './bind.ts';
import { loadDocument } from './load.ts';
import type { Field, MetricNode, Node, Reads, RecordNode, Section } from './types.ts';

// The page shows a call's time on the reader's clock; the fixtures are written in UTC.
(globalThis as unknown as { process: { env: Record<string, string | undefined> } }).process.env.TZ = 'UTC';
const ENV = (globalThis as unknown as { process: { env: Record<string, string | undefined> } }).process.env;
const DOC = loadDocument(YAML);
const COMMITTED = import.meta.glob<unknown>('../../ui/fixtures/**/*.json', { eager: true, import: 'default' });
const committed = (file: string) => COMMITTED[`../../ui/fixtures/${file}`];
const BASE = Date.UTC(2026, 10, 26, 9, 0, 0);

interface Site {
  path: string;
  reads: Reads;
  fields: string[];
}

// Every read of the document and the row fields the node reading it shows.
function sites(): Site[] {
  const out: Site[] = [];
  const fieldsOf = (n: Node | Section): string[] => {
    if ('host' in n || 'primitive' in n) return [];
    if (n.component === 'collection') return n.columns.map((f: Field) => f.field);
    if (n.component === 'record') {
      const r = n as RecordNode;
      const fromItems = (r.item ?? []).map((m) => (m as MetricNode).from?.match(/^row\.(\w+)$/)?.[1]).filter((f): f is string => !!f);
      return [...(r.fields ?? []).map((f) => f.field), ...fromItems];
    }
    if (n.component === 'metric') return n.aggregate && n.aggregate !== 'count' && n.field ? [n.field] : [];
    return [];
  };
  const walk = (path: string, n: Node | Section) => {
    const reads = (n as { reads?: Reads }).reads;
    if (reads) out.push({ path, reads, fields: fieldsOf(n) });
    const any = n as { expand?: Node; item?: Node[]; children?: Node[] };
    if (any.expand) walk(`${path}/expand`, any.expand);
    for (const c of any.item ?? []) walk(`${path}/item/${(c as { name?: string }).name}`, c);
    for (const c of any.children ?? []) walk(`${path}/children/${(c as { name?: string }).name}`, c);
  };
  for (const [name, page] of Object.entries(DOC.pages)) {
    for (const s of page.sections) walk(`pages/${name}/sections/${s.name}`, s);
    for (const m of page.header?.metrics ?? []) walk(`pages/${name}/header/metrics/${(m as { name?: string }).name}`, m);
  }
  return out;
}

function last(name: string): Snapshot {
  const states = SCENARIOS.find((s) => s.name === name)!.states(BASE);
  return states[states.length - 1]!;
}

// The rows a read answers for a snapshot; a read whose params read the row it stands in is asked
// once per call that has attempts.
function rowsFor(site: Site, snap: Snapshot) {
  const ph = site.reads.placeholder!;
  if (site.reads.params && 'call' in site.reads.params) {
    return snap.calls.filter((c) => c.attempts.length > 0).flatMap((c) => answer(ph, snap, snap.now_ms, { call: c.id }));
  }
  return answer(ph, snap, snap.now_ms, {});
}

test('every placeholder the document reads has a binding, and every binding is read', () => {
  const read = new Set(sites().map((s) => s.reads.placeholder));
  expect([...read].sort()).toEqual(Object.keys(BINDINGS).sort());
  expect(sites().filter((s) => !s.reads.placeholder || !s.reads.fixture)).toEqual([]);
});

for (const scenario of SCENARIOS.map((s) => s.name)) {
  test(`every row the binding answers holds each field the document reads from it — ${scenario}`, () => {
    const snap = last(scenario);
    const missing: string[] = [];
    let rows = 0;
    for (const site of sites()) {
      for (const row of rowsFor(site, snap)) {
        rows += 1;
        for (const f of site.fields) if (!(f in row)) missing.push(`${site.path}: ${f}`);
      }
    }
    expect([...new Set(missing)]).toEqual([]);
    expect(rows).toBeGreaterThan(0);
  });
}

test('the fixture files are what the binding answers for the last state of the errors scenario', async () => {
  const snap = last('errors');
  const want = new Map<string, unknown>();
  for (const site of sites()) {
    const file = site.reads.fixture!;
    if (want.has(file)) continue;
    want.set(file, JSON.parse(JSON.stringify({ view: site.reads.placeholder, rows: rowsFor(site, snap) })));
  }
  if (ENV.UPDATE_UI_FIXTURES === '1') {
    const fs = (await import(/* @vite-ignore */ String('node:fs'))) as { writeFileSync(path: URL, text: string): void };
    for (const [file, body] of want) fs.writeFileSync(new URL(`../../ui/fixtures/${file}`, import.meta.url), `${JSON.stringify(body, null, 2)}\n`);
    return;
  }
  for (const [file, body] of want) {
    expect(committed(file), `ui/fixtures/${file}; run the unit tests with UPDATE_UI_FIXTURES=1`).toBeDefined();
    expect(committed(file), `ui/fixtures/${file}`).toEqual(body);
  }
  expect(Object.keys(COMMITTED).filter((k) => !k.includes('/scripts/')).sort()).toEqual([...want.keys()].map((f) => `../../ui/fixtures/${f}`).sort());
  // Each channel's script of lifecycle changes.
  for (const [channel, file] of Object.entries(DOC.fixtures?.scripts ?? {})) {
    const script = committed(file) as { channel: string; events: { lifecycle?: string }[] };
    expect(script.channel).toBe(channel);
    expect(script.events.map((e) => e.lifecycle)).toEqual(['connecting', 'live']);
  }
});

test('a failed call is named by its status and kind of error, and the strip and Routes count the same calls', () => {
  const snap = last('errors');
  const sonnet = snap.calls.find((c) => c.route === 'claude-sonnet')!;
  expect(failureCause(sonnet)).toBe('429 rate limited');
  expect(failureCause({ ...sonnet, attempts: [], outcome: 'timeout', status: 504 })).toBe('504 timed out');
  expect(failureCause({ ...sonnet, attempts: [], outcome: 'refused', status: 404, refusal: 'unknown model' })).toBe('404 unknown model');
  expect(failedCall({ ...sonnet, status: 0, outcome: 'upstream_error', refusal: null })).toBe(false);
  expect(failedCall({ ...sonnet, client_disconnected: true })).toBe(false);
  const window = recentByRoute(snap, snap.now_ms).get('claude-sonnet')!;
  const line = answer('dashboard.Problems', snap, snap.now_ms, {}).find((r) => r.kind === 'routes')!;
  const row = answer('dashboard.Routes', snap, snap.now_ms, {}).find((r) => (r.route as { text: string }).text === 'claude-sonnet')!;
  expect(line.text).toContain(`claude-sonnet ${window.failed} of ${window.calls} failed, 429 rate limited`);
  expect((row.recent as { text: string }).text).toBe(`${window.failed} of ${window.calls}`);
});
