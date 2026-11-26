// @vitest-environment happy-dom
// The renderer over the document and its fixture files: the names the page is found by, the theme
// choice kept across loads, the expanded detail and the pager, and every primitive kind drawn.
import { afterEach, beforeEach, expect, test } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import YAML from '../../ui/llm-gateway.ui.yaml?raw';
import { HOSTS } from '../hosts.ts';
import EssApp from './components/EssApp.vue';
import { loadDocument } from './load.ts';
import { fixtureSource, type FixtureFile } from './source.ts';

const DOC = loadDocument(YAML);
const FILES = Object.entries(import.meta.glob<FixtureFile>('../../ui/fixtures/*.json', { eager: true, import: 'default' })).map(([, f]) => f);

beforeEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});
afterEach(() => {
  document.body.innerHTML = '';
});

const app = () => mount(EssApp, { props: { doc: DOC, page: 'dashboard', source: fixtureSource(FILES), hosts: HOSTS }, attachTo: document.body });

test('the page carries its headings, its table regions and the theme radio group', async () => {
  const w = app();
  await flushPromises();
  expect(w.find('h1').text()).toBe('llm-gateway');
  const headings = w.findAll('h2').map((h) => h.text().replace(/ \d+$/, ''));
  expect(headings).toEqual(expect.arrayContaining(['Overview', 'Provider', 'In flight', 'Routes', 'Sessions', 'Recent calls']));
  expect(w.find('#h-kpi').exists()).toBe(true);
  expect(w.find('#h-routes').exists()).toBe(true);
  expect(w.find('[role="region"][aria-label="Routes table"]').exists()).toBe(true);
  expect(w.find('[role="region"][aria-label="Recent calls table"]').exists()).toBe(true);
  const theme = w.find('[role="radiogroup"][aria-label="Theme"]');
  expect(theme.findAll('label').map((l) => l.text())).toEqual(['Light', 'Dark', 'System']);
  expect(w.find('.flow-panel').exists()).toBe(true);
  expect(w.find('header [role="status"]').text()).toBe('live');
  w.unmount();
});

test('the theme choice is applied on <html>, kept under llm-gateway.theme, and restored on the next load', async () => {
  const w = app();
  await flushPromises();
  expect(document.documentElement.dataset.theme).toBe('system');
  await w.find('input[type="radio"][value="dark"]').setValue(true);
  expect(document.documentElement.dataset.theme).toBe('dark');
  expect(localStorage.getItem('llm-gateway.theme')).toBe('dark');
  w.unmount();
  delete document.documentElement.dataset.theme;
  const again = app();
  await flushPromises();
  expect(document.documentElement.dataset.theme).toBe('dark');
  expect((again.find('input[type="radio"][value="dark"]').element as HTMLInputElement).checked).toBe(true);
  again.unmount();
  localStorage.setItem('llm-gateway.theme', 'sepia');
  const unknown = app();
  await flushPromises();
  expect(document.documentElement.dataset.theme).toBe('system');
  unknown.unmount();
});

test("a call's attempts open under its row, and the pager shows all calls and the last 30 again", async () => {
  const w = app();
  await flushPromises();
  const calls = w.find('table.calls');
  const toggle = calls.find('button[aria-expanded="false"]');
  expect(toggle.exists()).toBe(true);
  await toggle.trigger('click');
  expect(calls.find('button[aria-expanded="true"]').exists()).toBe(true);
  expect(calls.find('tr.attempts table.mini').exists()).toBe(true);
  // The fixture holds fewer calls than a page: no pager.
  expect(w.find('p.more').exists()).toBe(false);
  w.unmount();

  // Forty calls, the fixture's repeated under new ids: the page shows the last 30.
  const calls40 = FILES.find((f) => f.view === 'dashboard.Calls')!.rows;
  const many = Array.from({ length: 40 }, (_, i) => ({ ...calls40[i % calls40.length]!, id: 9000 - i, attempts: 0 }));
  const files = FILES.map((f) => (f.view === 'dashboard.Calls' ? { ...f, rows: many } : f));
  const paged = mount(EssApp, { props: { doc: DOC, page: 'dashboard', source: fixtureSource(files), hosts: HOSTS }, attachTo: document.body });
  await flushPromises();
  const rows = () => paged.find('table.calls').findAll('tbody > tr').length;
  expect(rows()).toBe(30);
  expect(paged.find('p.more button').text()).toBe('Show all 40');
  await paged.find('p.more button').trigger('click');
  expect(rows()).toBe(40);
  expect(paged.find('p.more button').text()).toBe('Show the last 30');
  paged.unmount();
});

const PRIMITIVES = `
format: ess-ui/1
app: probe
model: probe
theme: {default: system, chosen_by: shell.theme}
placement_profile: fat
shells:
  app:
    regions: {main: {kind: page_outlet}}
    state:
      compact: {type: boolean, class: preference, store: local_storage, default: false}
      theme: {type: {enum: [light, dark, system]}, class: preference, default: system}
navigation: {home: home, sections: [{name: all, pages: [home, other]}]}
pages:
  other: {kind: static_page, host: Other, reason: a page of its own, stands_in_for: none}
  home:
    kind: dashboard_page
    sections:
      - name: board
        remove: true
      - name: info
        title: Info
        component: record
        reads: {placeholder: probe.Info, fixture: info.json}
        fields: [name]
        children:
          - {name: hello, primitive: text, text: "'Hello'", style: heading}
          - {name: state, primitive: badge, text: section.info.row.state, tone_by: {value: section.info.row.state, map: {up: success}}}
          - {name: away, primitive: link, text: "'Other page'", to: {to: other}}
          - {name: dark, primitive: button, label: Dark, action: {sets: {shell.theme: "'dark'"}}, tone: primary}
          - {name: compact, primitive: toggle, label: Compact, binds: shell.compact}
`;

test('every primitive kind is drawn and its action runs', async () => {
  const doc = loadDocument(PRIMITIVES);
  const source = fixtureSource([{ view: 'probe.Info', rows: [{ name: 'probe', state: 'up' }] }]);
  const w = mount(EssApp, { props: { doc, page: 'home', source, hosts: { Other: { template: '<p>other</p>' } } }, attachTo: document.body });
  await flushPromises();
  expect(w.find('span.heading').text()).toBe('Hello');
  expect(w.find('.pill.tone-success').text()).toBe('up');
  expect(w.find('a[href="/other"]').text()).toBe('Other page');
  await w.find('button.action.emphasis-primary').trigger('click');
  expect(document.documentElement.dataset.theme).toBe('dark');
  const sw = w.find('button[role="switch"]');
  expect(sw.attributes('aria-checked')).toBe('false');
  await sw.trigger('click');
  expect(sw.attributes('aria-checked')).toBe('true');
  expect(localStorage.getItem('probe.compact')).toBe('true');
  w.unmount();
});
