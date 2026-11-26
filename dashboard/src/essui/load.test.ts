import YAML from '../../ui/llm-gateway.ui.yaml?raw';
import { expect, test } from 'vitest';
import { HOSTS } from '../hosts.ts';
import { loadDocument, tokensCss } from './load.ts';
import type { Document, Node, Section } from './types.ts';


// Every node of the document with its path, sections, expanded details, items, children and
// header metrics included.
function nodes(doc: Document): { path: string; node: Node | Section }[] {
  const out: { path: string; node: Node | Section }[] = [];
  const walk = (path: string, n: Node | Section) => {
    out.push({ path, node: n });
    const any = n as { expand?: Node; item?: Node[]; children?: Node[] };
    if (any.expand) walk(`${path}/expand`, any.expand);
    for (const c of any.item ?? []) walk(`${path}/item/${(c as { name?: string }).name}`, c);
    for (const c of any.children ?? []) walk(`${path}/children/${(c as { name?: string }).name}`, c);
  };
  for (const [name, page] of Object.entries(doc.pages)) {
    for (const s of page.sections) walk(`pages/${name}/sections/${s.name}`, s);
    for (const m of page.header?.metrics ?? []) walk(`pages/${name}/header/metrics`, m);
  }
  return out;
}

test('the document loads, and every node in it is a kind the renderer draws', () => {
  const doc = loadDocument(YAML);
  expect(doc.format).toBe('ess-ui/1');
  expect(Object.keys(doc.pages).sort()).toEqual(['dashboard', 'setup']);
  const kinds = new Set(nodes(doc).map(({ node }) => ('host' in node ? 'host' : 'primitive' in node ? node.primitive : node.component)));
  expect([...kinds].sort()).toEqual(['collection', 'host', 'link', 'metric', 'record', 'text']);
  // The page kind's `board` section is removed; the layout places every section.
  expect(doc.pages.dashboard!.sections.map((s) => s.name)).toEqual(['flow', 'kpi', 'limits', 'provider', 'pods', 'inflight', 'routes', 'sessions', 'calls', 'problems']);
});

test('a node kind, key, rendering or expression the renderer does not draw fails the load, named by its path', () => {
  const broken = (from: string, to: string) => () => loadDocument(YAML.replace(from, to));
  expect(broken('      - name: routes\n        title: Routes\n        component: collection', '      - name: routes\n        title: Routes\n        component: chart')).toThrow(
    "pages/dashboard/sections/routes: component 'chart' is not drawn by this renderer",
  );
  expect(broken("{name: total, component: metric, reads: {placeholder: dashboard.InFlight, fixture: in-flight.json}, aggregate: count}", '{name: total, primitive: icon, icon: clock, label: Clock}')).toThrow(
    "pages/dashboard/sections/inflight/children/total: primitive 'icon' is not drawn by this renderer",
  );
  expect(broken('        style: cards\n', '        style: cards\n        sort: {by: credential}\n')).toThrow("pages/dashboard/sections/limits: key 'sort' is not drawn by this renderer");
  expect(broken('{field: flags, label: Flags, as: tags}', '{field: flags, label: Flags, as: cron}')).toThrow("pages/dashboard/sections/calls/columns/flags: as 'cron' is not drawn");
  expect(broken('visible: row.window_5h}', 'visible: row.window_5h > 1}')).toThrow('pages/dashboard/sections/limits/columns/window_5h/visible');
  expect(broken('live: {channel: events, effect: replace}', 'live: {channel: events, effect: insert_top}')).toThrow("effect 'insert_top' is not drawn");
  expect(broken('      - name: board\n        remove: true\n', '')).toThrow("pages/dashboard/sections/board: component 'board' is not drawn by this renderer");
  expect(broken('        style: list\n', '        style: tree\n')).toThrow("pages/dashboard/sections/problems: style 'tree' is not drawn");
  expect(broken('{name: say, primitive: text,', '{name: say, primitive: image,')).toThrow("pages/dashboard/sections/problems/item/say: primitive 'image' is not drawn by this renderer");
});

test('every host slot of the document is registered by the page, and the page registers no other', () => {
  const doc = loadDocument(YAML);
  const slots = [...Object.values(doc.pages).map((p) => p.host), ...nodes(doc).map(({ node }) => ('host' in node ? node.host : undefined))].filter((h): h is string => !!h);
  expect(slots.sort()).toEqual(Object.keys(HOSTS).sort());
});

test("the tokens hold both themes' palettes, follow the browser under system, and give each tone its colours", () => {
  const css = tokensCss(loadDocument(YAML));
  expect(css).toMatch(/^:root\{[^}]*--bg:#f7f7f8;[^}]*--surface:#ffffff;[^}]*color-scheme:light\}/);
  expect(css).toMatch(/:root\[data-theme='dark'\]\{[^}]*--bg:#121417;[^}]*--flow_out:#fb923c;[^}]*color-scheme:dark\}/);
  expect(css).toMatch(/@media \(prefers-color-scheme: dark\)\{:root\[data-theme='system'\],:root:not\(\[data-theme\]\)\{[^}]*--bg:#121417/);
  expect(css).toContain('.tone-danger{--tone-text:var(--danger);--tone-fill:var(--danger_fill)}');
  expect(css).toContain('--type-mono-family:ui-monospace');
  expect(() => tokensCss({ ...loadDocument(YAML), tokens: { color: { bg: 'red;}body{display:none' } } })).toThrow('a stylesheet cannot hold');
});
