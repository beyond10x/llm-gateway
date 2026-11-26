// Loads the ess-ui/1 document at build time: parses the YAML, expands the short forms this renderer
// reads, merges each page with its page kind, and refuses every node kind, key, field rendering,
// layout and expression the renderer does not draw. The Vite plugin in vite.config.ts calls it, so a
// document the renderer cannot draw fails the build; src/essui/load.test.ts holds the same check.
import { parse as parseYaml } from 'yaml';
import { parse as parseExpr } from './expr.ts';
import type { Document, Tokens, Tone } from './types.ts';

type Obj = Record<string, unknown>;

export const COMPOSITES = ['collection', 'record', 'metric', 'choice'] as const;
export const PRIMITIVES = ['text', 'badge', 'link', 'button', 'toggle'] as const;
export const FIELD_AS = ['text', 'number', 'duration', 'percent', 'badge', 'tags', 'map', 'json'] as const;
export const TONES: Tone[] = ['neutral', 'info', 'success', 'warning', 'danger'];

const DOCUMENT_KEYS = ['format', 'app', 'title', 'model', 'actor', 'placement_profile', 'fixtures', 'tokens', 'themes', 'theme', 'tone_maps', 'label_maps', 'shells', 'navigation', 'channels', 'pages'];
const PAGE_KEYS = ['kind', 'title', 'header', 'layout', 'sections'];
const HOST_KEYS = ['name', 'host', 'reason', 'stands_in_for', 'reads'];
const HEADER_KEYS = ['title', 'live', 'actions', 'metrics'];
const ACTION_KEYS = ['name', 'label', 'as', 'navigate', 'sets', 'choice'];
const SECTION_KEYS = ['name', 'title', 'visible', 'live', 'states', 'children'];
const COMPOSITE_KEYS: Record<(typeof COMPOSITES)[number], string[]> = {
  collection: ['component', 'name', 'reads', 'live', 'visible', 'style', 'columns', 'expand'],
  record: ['component', 'name', 'reads', 'live', 'visible', 'fields', 'item'],
  metric: ['component', 'name', 'reads', 'live', 'visible', 'from', 'aggregate', 'field', 'format', 'unit', 'window', 'label'],
  choice: ['component', 'name', 'binds', 'style', 'options', 'visible'],
};
const PRIMITIVE_KEYS: Record<(typeof PRIMITIVES)[number], string[]> = {
  text: ['primitive', 'name', 'text', 'style', 'visible'],
  badge: ['primitive', 'name', 'text', 'tone', 'tone_by', 'visible'],
  link: ['primitive', 'name', 'text', 'to', 'href', 'visible'],
  button: ['primitive', 'name', 'label', 'action', 'tone', 'visible'],
  toggle: ['primitive', 'name', 'label', 'binds', 'visible'],
};
const FIELD_KEYS = ['field', 'name', 'label', 'as', 'tone', 'tone_by', 'labels', 'visible', 'help'];
const READS_KEYS = ['view', 'placeholder', 'fixture', 'key', 'params', 'paging', 'pager'];

// The sections each built-in page kind gives a page, by name; a page removes or replaces them.
const KIND_SECTIONS: Record<string, Obj[]> = {
  dashboard_page: [{ name: 'board', component: 'board' }],
  static_page: [{ name: 'body', component: 'record' }],
  detail_page: [{ name: 'summary', component: 'record' }],
};

const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v);

class Problems {
  readonly list: string[] = [];
  add(path: string, what: string) {
    this.list.push(`${path}: ${what}`);
  }
  keys(path: string, node: Obj, allowed: string[]) {
    for (const k of Object.keys(node)) if (!allowed.includes(k)) this.add(path, `key '${k}' is not drawn by this renderer`);
  }
  expr(path: string, src: unknown) {
    if (typeof src !== 'string') return this.add(path, 'an expression must be a string');
    try {
      parseExpr(src);
    } catch (e) {
      this.add(path, (e as Error).message);
    }
  }
}

// A field written as a bare name is `{field: name, name: name}`; a field's name defaults to it.
function expandField(f: unknown): unknown {
  if (typeof f === 'string') return { field: f, name: f };
  if (isObj(f) && typeof f.field === 'string' && f.name === undefined) return { ...f, name: f.field };
  return f;
}

function mergeSections(kind: string, own: unknown[]): Obj[] {
  const out: Obj[] = (KIND_SECTIONS[kind] ?? []).map((s) => ({ ...s }));
  for (const s of own) {
    if (!isObj(s)) continue;
    const at = out.findIndex((o) => o.name === s.name);
    if (s.remove === true) {
      if (at >= 0) out.splice(at, 1);
    } else if (at >= 0) {
      out[at] = out[at]!.component === s.component ? { ...out[at], ...s } : s;
    } else {
      out.push(s);
    }
  }
  return out;
}

function checkReads(p: Problems, path: string, reads: unknown) {
  if (!isObj(reads)) return p.add(path, 'reads must be a record');
  p.keys(path, reads, READS_KEYS);
  if (reads.view !== undefined) p.add(path, 'a view of a model: this renderer answers placeholders only');
  if (typeof reads.placeholder !== 'string') p.add(path, 'reads names no placeholder');
  if (reads.paging !== undefined && reads.paging !== 'client' && reads.paging !== 'none') p.add(path, `paging '${String(reads.paging)}' is not drawn`);
  if (reads.pager !== undefined && (!isObj(reads.pager) || Object.keys(reads.pager).some((k) => k !== 'sizes'))) p.add(path, 'a pager names its sizes only');
  for (const [k, v] of Object.entries(isObj(reads.params) ? reads.params : {})) p.expr(`${path}/params/${k}`, v);
}

function checkLive(p: Problems, path: string, live: unknown, doc: Obj) {
  if (!isObj(live)) return p.add(path, 'live must be a record');
  p.keys(path, live, ['channel', 'effect']);
  if (live.effect !== 'replace') p.add(path, `effect '${String(live.effect)}' is not drawn: every read is replaced by the channel's value`);
  if (!isObj(doc.channels) || !(String(live.channel) in doc.channels)) p.add(path, `channel '${String(live.channel)}' is not declared`);
}

function checkField(p: Problems, path: string, f: unknown, doc: Obj) {
  if (!isObj(f)) return p.add(path, 'a field must be a name or a record');
  p.keys(path, f, FIELD_KEYS);
  if (f.as !== undefined && !(FIELD_AS as readonly string[]).includes(String(f.as))) p.add(path, `as '${String(f.as)}' is not drawn`);
  if (f.tone_by !== undefined && !(isObj(doc.tone_maps) && String(f.tone_by) in doc.tone_maps)) p.add(path, `tone map '${String(f.tone_by)}' is not declared`);
  if (typeof f.labels === 'string' && !(isObj(doc.label_maps) && f.labels in doc.label_maps)) p.add(path, `label map '${f.labels}' is not declared`);
  if (f.visible !== undefined) p.expr(`${path}/visible`, f.visible);
}

function checkAction(p: Problems, path: string, a: unknown, doc: Obj) {
  if (!isObj(a)) return p.add(path, 'an action must be a record');
  p.keys(path, a, ACTION_KEYS);
  const does = ['navigate', 'sets'].filter((k) => a[k] !== undefined);
  if (does.length !== 1) p.add(path, 'an action navigates or sets state, exactly one of the two');
  if (isObj(a.navigate) && !(isObj(doc.pages) && String(a.navigate.to) in doc.pages)) p.add(path, `page '${String(a.navigate.to)}' is not declared`);
  if (isObj(a.sets)) {
    for (const [k, v] of Object.entries(a.sets)) {
      p.expr(`${path}/sets`, k);
      if (v !== 'toggle') p.expr(`${path}/sets/${k}`, v);
    }
  }
  if (a.as === 'choice') {
    if (!isObj(a.choice)) p.add(path, 'an action drawn as a choice needs its choice');
    else checkNode(p, `${path}/choice`, a.choice, doc);
  } else if (a.as !== undefined && a.as !== 'link' && a.as !== 'button' && a.as !== 'toggle') {
    p.add(path, `as '${String(a.as)}' is not drawn`);
  }
}

function checkNode(p: Problems, path: string, node: unknown, doc: Obj, sectionKeys: string[] = []) {
  if (!isObj(node)) return p.add(path, 'a node must be a record');
  if (node.host !== undefined) {
    p.keys(path, node, [...HOST_KEYS, ...sectionKeys]);
    if (typeof node.reason !== 'string' || node.reason.trim() === '') p.add(path, 'a host slot gives its reason');
    if (node.reads !== undefined) checkReads(p, `${path}/reads`, node.reads);
    return;
  }
  if (node.primitive !== undefined) {
    const kind = String(node.primitive) as (typeof PRIMITIVES)[number];
    if (!PRIMITIVES.includes(kind)) return p.add(path, `primitive '${kind}' is not drawn by this renderer`);
    p.keys(path, node, PRIMITIVE_KEYS[kind]);
    if (node.visible !== undefined) p.expr(`${path}/visible`, node.visible);
    if (kind === 'button') checkAction(p, `${path}/action`, node.action, doc);
    if (kind === 'toggle') p.expr(`${path}/binds`, node.binds);
    if (kind === 'badge' && isObj(node.tone_by)) {
      p.expr(`${path}/tone_by/value`, node.tone_by.value);
      if (!isObj(node.tone_by.map)) p.add(path, 'a badge here writes its tone map out as map; tones is not resolved');
    }
    return;
  }
  const kind = String(node.component) as (typeof COMPOSITES)[number];
  if (!COMPOSITES.includes(kind)) return p.add(path, `component '${kind}' is not drawn by this renderer`);
  p.keys(path, node, [...COMPOSITE_KEYS[kind], ...sectionKeys]);
  if (node.visible !== undefined) p.expr(`${path}/visible`, node.visible);
  if (node.reads !== undefined) checkReads(p, `${path}/reads`, node.reads);
  if (node.live !== undefined) checkLive(p, `${path}/live`, node.live, doc);
  switch (kind) {
    case 'collection':
      if (node.style !== undefined && node.style !== 'table' && node.style !== 'cards') p.add(path, `style '${String(node.style)}' is not drawn`);
      if (!Array.isArray(node.columns)) p.add(path, 'a collection lists its columns');
      else node.columns.forEach((f, i) => checkField(p, `${path}/columns/${isObj(f) ? String(f.name) : i}`, f, doc));
      if (node.expand !== undefined) checkNode(p, `${path}/expand`, node.expand, doc);
      break;
    case 'record':
      (Array.isArray(node.fields) ? node.fields : []).forEach((f, i) => checkField(p, `${path}/fields/${isObj(f) ? String(f.name) : i}`, f, doc));
      (Array.isArray(node.item) ? node.item : []).forEach((n, i) => checkNode(p, `${path}/item/${isObj(n) ? String(n.name) : i}`, n, doc));
      break;
    case 'metric':
      if (node.from !== undefined) p.expr(`${path}/from`, node.from);
      if (node.format !== undefined && !['number', 'duration', 'percent'].includes(String(node.format))) p.add(path, `format '${String(node.format)}' is not drawn`);
      if (node.from === undefined && node.aggregate === undefined) p.add(path, 'a metric takes its value from an expression or an aggregate');
      break;
    case 'choice':
      p.expr(`${path}/binds`, node.binds);
      if (!Array.isArray(node.options) || node.options.some((o) => !isObj(o))) p.add(path, 'a choice lists its options as {value, label}');
      break;
  }
}

function checkLayout(p: Problems, path: string, layout: unknown, sections: Obj[]) {
  if (layout === undefined || layout === 'stack') return;
  if (!isObj(layout) || !isObj(layout.areas) || Object.keys(layout).length !== 1) return p.add(path, 'only stack and areas are drawn');
  const { grid, place } = layout.areas as Obj;
  if (!Array.isArray(grid) || grid.some((row) => !Array.isArray(row) || row.length !== 1)) p.add(`${path}/areas/grid`, 'every row of the grid is one area: the page is drawn as one column');
  const placed: string[] = [];
  for (const [area, v] of Object.entries(isObj(place) ? place : {})) {
    const list = Array.isArray(v) ? v : isObj(v) ? v.sections : undefined;
    if (!Array.isArray(list)) p.add(`${path}/areas/place/${area}`, 'an area lists its sections');
    else placed.push(...list.map(String));
    if (isObj(v)) p.keys(`${path}/areas/place/${area}`, v, ['sections', 'frame', 'title']);
  }
  for (const s of sections) if (!placed.includes(String(s.name))) p.add(path, `section '${String(s.name)}' is placed in no area`);
}

// The document as the renderer reads it, or an error listing every node it cannot draw.
export function loadDocument(yamlText: string): Document {
  const raw = parseYaml(yamlText) as unknown;
  if (!isObj(raw)) throw new Error('the ess-ui document is not a record');
  const p = new Problems();
  if (raw.format !== 'ess-ui/1') p.add('format', "must be 'ess-ui/1'");
  p.keys('', raw, DOCUMENT_KEYS);
  const pages: Obj = {};
  for (const [name, page] of Object.entries(isObj(raw.pages) ? raw.pages : {})) {
    const path = `pages/${name}`;
    if (!isObj(page)) {
      p.add(path, 'a page must be a record');
      continue;
    }
    if (page.host !== undefined) {
      p.keys(path, page, ['kind', 'title', ...HOST_KEYS]);
      pages[name] = { ...page, sections: [] };
      continue;
    }
    p.keys(path, page, PAGE_KEYS);
    if (!(String(page.kind) in KIND_SECTIONS)) p.add(path, `page kind '${String(page.kind)}' is not drawn`);
    const sections = mergeSections(String(page.kind), Array.isArray(page.sections) ? page.sections : []).map((s) =>
      isObj(s) && Array.isArray(s.columns) ? { ...s, columns: s.columns.map(expandField) } : s,
    );
    for (const s of sections) {
      if (isObj(s.expand) && Array.isArray(s.expand.columns)) s.expand = { ...s.expand, columns: s.expand.columns.map(expandField) };
      if (Array.isArray(s.fields)) s.fields = s.fields.map(expandField);
    }
    sections.forEach((s) => {
      const at = `${path}/sections/${String(s.name)}`;
      checkNode(p, at, s, raw, SECTION_KEYS);
      if (isObj(s.states)) p.keys(`${at}/states`, s.states, ['empty']);
      (Array.isArray(s.children) ? s.children : []).forEach((n, i) => checkNode(p, `${at}/children/${isObj(n) ? String(n.name) : i}`, n, raw));
    });
    if (isObj(page.header)) {
      p.keys(`${path}/header`, page.header, HEADER_KEYS);
      for (const c of Array.isArray(page.header.live) ? page.header.live : []) {
        if (!isObj(raw.channels) || !(String(c) in raw.channels)) p.add(`${path}/header/live`, `channel '${String(c)}' is not declared`);
      }
      (Array.isArray(page.header.actions) ? page.header.actions : []).forEach((a, i) => checkAction(p, `${path}/header/actions/${isObj(a) ? String(a.name) : i}`, a, raw));
      (Array.isArray(page.header.metrics) ? page.header.metrics : []).forEach((n, i) => checkNode(p, `${path}/header/metrics/${isObj(n) ? String(n.name) : i}`, n, raw));
    }
    checkLayout(p, `${path}/layout`, page.layout, sections);
    pages[name] = { ...page, sections };
  }
  for (const [name, shell] of Object.entries(isObj(raw.shells) ? raw.shells : {})) {
    if (!isObj(shell)) continue;
    p.keys(`shells/${name}`, shell, ['regions', 'state']);
    for (const [r, region] of Object.entries(isObj(shell.regions) ? shell.regions : {})) {
      if (!isObj(region) || region.kind !== 'page_outlet') p.add(`shells/${name}/regions/${r}`, 'only a page_outlet region is drawn');
    }
  }
  if (p.list.length > 0) throw new Error(`the ess-ui document holds what this renderer does not draw:\n${p.list.join('\n')}`);
  return { ...raw, pages } as unknown as Document;
}

// The built-in token table of ess-ui/1, which a document's `tokens:` is merged over.
export const BUILTIN_TOKENS: Required<Tokens> = {
  color: {
    bg: '#f7f7f8', surface: '#ffffff', text: '#1c1d21', muted: '#676a73', line: '#dcdde1', accent: '#2f5bd3', info: '#2f7fd3',
    success: '#2e8a4f', warning: '#b7791f', danger: '#c0392b', danger_fill: '#f6d5d1', success_fill: '#d7f0df', warning_fill: '#f7e7c6',
    info_fill: '#d9e4fb', focus: '#eef2fd', on_accent: '#fff', backdrop: 'rgb(0 0 0 / 0.3)',
  },
  space: { xs: '0.25rem', sm: '0.5rem', md: '1rem', lg: '1.5rem' },
  radius: { sm: '4px', md: '6px', lg: '8px', pill: '999px' },
  type: { body: { family: 'system-ui, sans-serif' }, caption: { size: '0.8rem' }, heading: { size: '1.05rem', weight: 600 }, mono: { family: 'ui-monospace, monospace' } },
  tone: {
    neutral: { text: 'text', fill: 'line' },
    info: { text: 'info', fill: 'line' },
    success: { text: 'success', fill: 'line' },
    warning: { text: 'warning', fill: 'line' },
    danger: { text: 'danger', fill: 'danger_fill' },
  },
};

const SAFE = /^[^;{}<>\\]*$/;

function declarations(t: Tokens): string[] {
  const out: string[] = [];
  const put = (name: string, v: string | number | undefined) => {
    if (v === undefined) return;
    if (!/^[A-Za-z0-9_-]+$/.test(name) || !SAFE.test(String(v))) throw new Error(`token ${name} has a value a stylesheet cannot hold: ${String(v)}`);
    out.push(`--${name}:${String(v)}`);
  };
  for (const [k, v] of Object.entries(t.color ?? {})) put(k, v);
  for (const [k, v] of Object.entries(t.space ?? {})) put(`space-${k}`, v);
  for (const [k, v] of Object.entries(t.radius ?? {})) put(`radius-${k}`, v);
  for (const [style, v] of Object.entries(t.type ?? {})) {
    put(`type-${style}-family`, v?.family);
    put(`type-${style}-size`, v?.size);
    put(`type-${style}-weight`, v?.weight);
  }
  return out;
}

// The tokens as custom properties on :root, each theme's overrides under [data-theme], and one
// class per tone that sets --tone-text and --tone-fill. A theme named `system` follows the
// browser: the base look, and the theme named `dark` under `prefers-color-scheme: dark`.
export function tokensCss(doc: Document): string {
  const t = doc.tokens ?? {};
  const base: Tokens = {
    color: { ...BUILTIN_TOKENS.color, ...t.color },
    space: { ...BUILTIN_TOKENS.space, ...t.space },
    radius: { ...BUILTIN_TOKENS.radius, ...t.radius },
    type: { ...BUILTIN_TOKENS.type, ...t.type },
  };
  const tone = { ...BUILTIN_TOKENS.tone, ...t.tone };
  const scheme = (name: string) => (name === 'dark' ? 'dark' : 'light');
  const css = [`:root{${declarations(base).join(';')};color-scheme:light}`];
  const themes = doc.themes ?? {};
  for (const [name, overrides] of Object.entries(themes)) {
    const decl = declarations(overrides);
    css.push(`:root[data-theme='${name}']{${[...decl, `color-scheme:${scheme(name)}`].join(';')}}`);
  }
  if ('system' in themes && themes.dark) {
    const decl = declarations(themes.dark);
    css.push(`@media (prefers-color-scheme: dark){:root[data-theme='system'],:root:not([data-theme]){${[...decl, 'color-scheme:dark'].join(';')}}}`);
  }
  for (const name of TONES) {
    const v = tone[name];
    if (v) css.push(`.tone-${name}{--tone-text:var(--${v.text});--tone-fill:var(--${v.fill})}`);
  }
  return css.join('\n');
}
