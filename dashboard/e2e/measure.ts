// One measurement of one scenario at one viewport in one theme: replays every state, probes the
// layout, the always-visible elements and the key data and runs axe-core after each, measures the
// token-flow canvas, walks the page by keyboard and replays again with reduced motion. And the
// lighter pass of the measured-only viewports: the always-visible elements at every state.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Browser, Page } from '@playwright/test';
import type { Scenario } from '../ux/scenarios/index.ts';
import { runAxe } from './axe.ts';
import { documentLabels } from './document.ts';
import { fontsLabel } from './fonts.ts';
import { openReplay, settle, type Replay } from './harness.ts';
import { axeRules, headingFontDeficit, keyFontDeficit, mergeAxe, secondaryFontDeficit, type AxeFinding, type Metrics } from './metrics.ts';
import { runId } from './metrics-dir.ts';
import {
  ALWAYS_ATTR,
  canvasPixels,
  layoutProbe,
  flowProbe,
  focusStep,
  headerProbeSource,
  markCandidates,
  motionProbe,
  stateProbeSource,
  type FlowProbe,
  type FontItem,
  type HeaderItem,
  type StateProbe,
} from './probe.ts';
import { AXE_TAGS, type Theme, type Viewport } from './profiles.ts';
import { screenOnly, type TaskResult } from './screen-only.ts';

export { METRIC_KEYS, type AxeFinding, type Metrics } from './metrics.ts';

// An element found by a probe: its stable selector (the key known failures name) and a label.
export interface Item {
  key: string;
  label: string;
}

export interface Measurement {
  run: string;
  scenario: string;
  viewport: Viewport;
  theme: Theme;
  toggle: 'absent' | 'present';
  // The emulated colour scheme shows its palette with the toggle on its default.
  theme_applied: boolean;
  states: number;
  metrics: Metrics;
  // Distinct nodes per axe rule over every scan.
  axe_rules: Record<string, number>;
  axe_scans: number;
  // null when the page shows no key data (no secondary text).
  key_font_min_px: number | null;
  key_font_at: string;
  heading_font_min_px: number | null;
  heading_font_at: string;
  secondary_font_min_px: number | null;
  secondary_font_at: string;
  header_min_visible_ratio: number;
  shifts: number[];
  // The connection status text at every state, then after every action.
  status: string[];
  snapshot_fetches: number;
  blocked_requests: number;
  blocked: string[];
  // The token-flow canvas at the last state, scrolled to the top, and at the last state of the
  // reduced-motion replay.
  flow: FlowProbe;
  reduced_flow: FlowProbe;
  // The screen-only user's tasks (e2e/screen-only.ts), played in the light theme only (the layout
  // does not depend on the theme); empty in the dark theme and for a scenario without a task.
  screen_only: TaskResult[];
  // The fonts the replay rendered with (e2e/fonts.ts).
  tools: { chromium: string; axe_core: string; fonts: string };
  details: {
    axe: AxeFinding[];
    overflowing: Item[];
    scrollers: Item[];
    scrollers_unreachable: string[];
    truncated: Item[];
    header: HeaderItem[];
    unreachable: string[];
    focus_invisible: string[];
    reduced_motion: string[];
    key_font_below: string[];
    heading_font_below: string[];
    secondary_font_below: string[];
    // The visible-text probe's items over every state and action (e2e/copy.ts).
    copy: Item[];
  };
}

// The always-visible elements of one scenario in one theme at the measured-only viewports, at
// every state.
export interface HeaderMeasurement {
  run: string;
  scenario: string;
  theme: Theme;
  viewports: Record<string, HeaderItem[]>;
  // The elements that scroll sideways at each viewport, over every state and action.
  scrollers: Record<string, Item[]>;
}

const round3 = (x: number) => Math.round(x * 1000) / 1000;
const STATE_PROBE = stateProbeSource(documentLabels());
const HEADER_PROBE = headerProbeSource();

async function markAlways(page: Page, replay: Replay): Promise<void> {
  const h1 = page.locator('h1').first();
  if ((await h1.count()) > 0) await h1.evaluate((el, a) => el.setAttribute(a, 'h1'), ALWAYS_ATTR);
  const status = page.locator('header [role="status"], [role="status"]').first();
  if ((await status.count()) > 0) await status.evaluate((el, a) => el.setAttribute(a, 'connection status'), ALWAYS_ATTR);
  if (replay.toggle) await replay.toggle.evaluate((el, a) => el.setAttribute(a, 'theme toggle'), ALWAYS_ATTR);
}

async function axe(page: Page): Promise<{ findings: AxeFinding[]; version: string }> {
  const result = await runAxe(page, { tags: AXE_TAGS });
  const findings = result.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => {
      const targets = [...new Set(v.nodes.map((n) => n.target.join(' ')))].sort();
      return { id: v.id, impact: v.impact ?? '', help: v.help, nodes: targets.length, targets };
    });
  return { findings, version: result.version };
}

// Keeps the smallest visible share of each always-visible element.
function keepHeader(into: Map<string, number>, items: HeaderItem[]): void {
  for (const h of items) into.set(h.name, Math.min(into.get(h.name) ?? 1, h.visible_ratio));
}

const headerList = (m: Map<string, number>): HeaderItem[] => [...m.entries()].map(([name, visible_ratio]) => ({ name, visible_ratio: round3(visible_ratio) }));

async function keyboardWalk(page: Page): Promise<{ unreachable: string[]; invisible: string[]; scrollersUnreachable: string[] }> {
  const candidates = await page.evaluate(markCandidates);
  // Start the walk at the top of the document: focusing a probe element placed before everything
  // else moves the sequential focus starting point there, whatever was clicked before.
  await page.evaluate(() => {
    (document.activeElement as HTMLElement | null)?.blur();
    const start = document.createElement('span');
    start.tabIndex = -1;
    start.id = 'ux-walk-start';
    document.body.prepend(start);
    start.focus();
    window.scrollTo(0, 0);
  });
  const reached = new Set<number>();
  const invisible = new Set<string>();
  const limit = candidates.length + 10;
  let left = false;
  for (let i = 0; i < limit; i++) {
    await page.keyboard.press('Tab');
    const step = await page.evaluate(focusStep);
    if (step.index === null && step.inside.length === 0) {
      if (left || i > 0) break;
      continue;
    }
    left = true;
    if (step.index !== null) {
      if (reached.has(step.index)) break;
      reached.add(step.index);
    }
    for (const p of step.inside) reached.add(-1 - p);
    if (!step.visible) invisible.add(`${step.label}: ${step.why}`);
  }
  await page.evaluate(() => document.getElementById('ux-walk-start')?.remove());
  const unreachable: string[] = [];
  const scrollersUnreachable: string[] = [];
  const groupsReached = new Set(candidates.filter((c) => c.group !== null && reached.has(c.index)).map((c) => c.group));
  for (const c of candidates) {
    if (c.group !== null && groupsReached.has(c.group)) continue;
    if (c.scroller) {
      // A scroller is reachable when it takes focus itself or holds a control that does.
      if (!reached.has(c.index) && !reached.has(-1 - c.index)) scrollersUnreachable.push(c.label);
    } else if (!reached.has(c.index)) {
      unreachable.push(c.label);
    }
  }
  return { unreachable, invisible: [...invisible], scrollersUnreachable };
}

// `shots.last`: the viewport at the last state, 0.75 s after it arrived; `shots.full`: the whole page
// after the last action.
export async function measure(browser: Browser, scenario: Scenario, viewport: Viewport, theme: Theme, shots: { last?: string; full?: string } = {}): Promise<Measurement> {
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, timezoneId: 'UTC', locale: 'en-GB' });
  const page = await context.newPage();
  try {
    // The reduced-motion replay runs in a context of its own beside this one; it mostly waits.
    const reducedMotionRun = reducedMotion(browser, scenario, viewport, theme);
    reducedMotionRun.catch(() => undefined);
    const screenOnlyRun = theme === 'light' ? screenOnly(browser, scenario, viewport, theme) : Promise.resolve([]);
    screenOnlyRun.catch(() => undefined);
    const replay = await openReplay(page, scenario, theme);
    await markAlways(page, replay);

    // Keyed by element selector, so an element counts once over every state.
    const overflowing = new Map<string, string>();
    const scrollers = new Map<string, string>();
    const truncated = new Map<string, string>();
    const copy = new Map<string, string>();
    const keep = (into: Map<string, string>) => (entry: string) => {
      const [key, label = entry] = entry.split(' | ');
      if (!into.has(key!)) into.set(key!, label);
    };
    const header = new Map<string, number>();
    let overflowPx = 0;
    let keyFont: FontItem | null = null;
    let headingFont: FontItem | null = null;
    const headingBelow = new Set<string>();
    let secondaryFont: FontItem | null = null;
    const keyBelow = new Set<string>();
    const secondaryBelow = new Set<string>();
    const shifts: number[] = [];
    const status: string[] = [];
    const axeFindings = new Map<string, AxeFinding>();
    let axeScans = 0;
    let axeVersion = '';

    const probe = async () => {
      const state = await page.evaluate<StateProbe>(STATE_PROBE);
      for (const { layout: p, header: h } of state.views) {
        overflowPx = Math.max(overflowPx, p.overflow_x_px);
        p.overflowing.forEach(keep(overflowing));
        p.scrollers.forEach(keep(scrollers));
        p.truncated.forEach(keep(truncated));
        keepHeader(header, h);
      }
      const k = state.key_data;
      if (k.key && (!keyFont || k.key.px < keyFont.px)) keyFont = k.key;
      if (k.heading && (!headingFont || k.heading.px < headingFont.px)) headingFont = k.heading;
      k.heading_below.forEach((x) => headingBelow.add(x));
      state.copy.forEach(keep(copy));
      if (k.secondary && (!secondaryFont || k.secondary.px < secondaryFont.px)) secondaryFont = k.secondary;
      k.key_below.forEach((x) => keyBelow.add(x));
      k.secondary_below.forEach((x) => secondaryBelow.add(x));
    };
    // axe-core after every state and every action; the union of nodes per rule.
    const scan = async () => {
      const r = await axe(page);
      mergeAxe(axeFindings, r.findings);
      axeVersion = r.version;
      axeScans += 1;
    };

    status.push(await replay.status());
    await probe();
    await scan();
    while (replay.index + 1 < replay.states.length) {
      await page.evaluate(() => window.scrollTo(0, 0));
      await replay.takeShift();
      await replay.advance();
      shifts.push(round3(await replay.takeShift()));
      status.push(await replay.status());
      await probe();
      await scan();
    }
    await settle(page);
    const flow = await page.evaluate(flowProbe);
    if (shots.last) {
      await page.waitForTimeout(750);
      mkdirSync(join(shots.last, '..'), { recursive: true });
      await page.screenshot({ path: shots.last, animations: 'allow' });
    }
    for (const action of scenario.actions ?? []) {
      await replay.act(action);
      await page.mouse.move(0, 0);
      status.push(await replay.status());
      await probe();
      await scan();
    }
    if (shots.full) {
      await page.evaluate(() => window.scrollTo(0, 0));
      await settle(page);
      mkdirSync(join(shots.full, '..'), { recursive: true });
      await page.screenshot({ path: shots.full, fullPage: true, animations: 'allow' });
    }
    const walk = await keyboardWalk(page);
    const motion = await reducedMotionRun;
    const tasks = await screenOnlyRun;

    const headerItems = headerList(header);
    const headerMin = headerItems.length ? Math.min(...headerItems.map((h) => h.visible_ratio)) : 0;
    const axeList = [...axeFindings.values()].sort((a, b) => a.id.localeCompare(b.id));
    const fontOf = (f: FontItem | null) => (f ? round3(f.px) : null);
    const key = keyFont as FontItem | null;
    const secondary = secondaryFont as FontItem | null;
    const heading = headingFont as FontItem | null;
    const items = (m: Map<string, string>): Item[] => [...m.entries()].map(([k, label]) => ({ key: k, label }));
    const metrics: Metrics = {
      'axe.serious_critical': axeList.reduce((a, f) => a + f.nodes, 0),
      'overflow.x_px': Math.round(overflowPx),
      'overflow.elements': overflowing.size,
      'overflow.scrollers': scrollers.size,
      'overflow.scrollers_unreachable': walk.scrollersUnreachable.length,
      'header.hidden_ratio': round3(1 - headerMin),
      'keyboard.unreachable': walk.unreachable.length,
      'keyboard.focus_invisible': walk.invisible.length,
      'key_font.deficit_px': keyFontDeficit(fontOf(key)),
      'heading_font.deficit_px': headingFontDeficit(fontOf(heading)),
      'secondary_font.deficit_px': secondaryFontDeficit(fontOf(secondary)),
      'layout_shift.per_state_max': shifts.length ? Math.max(...shifts) : 0,
      'reduced_motion.violations': motion.violations.length,
      'truncation.without_full_value': truncated.size,
      'copy.defects': copy.size,
    };
    return {
      run: runId(),
      scenario: scenario.name,
      viewport,
      theme,
      toggle: replay.toggle ? 'present' : 'absent',
      theme_applied: replay.themeApplied,
      states: replay.states.length,
      metrics,
      axe_rules: axeRules(axeList),
      axe_scans: axeScans,
      key_font_min_px: fontOf(key),
      key_font_at: key?.at ?? '',
      heading_font_min_px: fontOf(heading),
      heading_font_at: heading?.at ?? '',
      secondary_font_min_px: fontOf(secondary),
      secondary_font_at: secondary?.at ?? '',
      header_min_visible_ratio: headerMin,
      shifts,
      status,
      snapshot_fetches: replay.snapshotFetches,
      blocked_requests: replay.blocked.length,
      blocked: [...replay.blocked],
      flow,
      reduced_flow: motion.flow,
      screen_only: tasks,
      tools: { chromium: browser.version(), axe_core: axeVersion, fonts: fontsLabel() },
      details: {
        axe: axeList,
        overflowing: items(overflowing),
        scrollers: items(scrollers),
        scrollers_unreachable: walk.scrollersUnreachable,
        truncated: items(truncated),
        header: headerItems,
        unreachable: walk.unreachable,
        focus_invisible: walk.invisible,
        reduced_motion: motion.violations,
        key_font_below: [...keyBelow].sort(),
        heading_font_below: [...headingBelow].sort(),
        secondary_font_below: [...secondaryBelow].sort(),
        copy: items(copy).sort((a, b) => a.key.localeCompare(b.key)),
      },
    };
  } finally {
    await context.close();
  }
}

export interface ReducedMotion {
  violations: string[];
  // The token-flow canvas at the last state.
  flow: FlowProbe;
}

// With prefers-reduced-motion: reduce, no CSS animation or transition runs while the replay steps,
// every canvas keeps the same pixels over 500 ms, and the token-flow canvas at the last state is
// measured as in the main replay: a static drawing, not a blank one.
export async function reducedMotionReplay(page: Page, scenario: Scenario, theme: Theme): Promise<ReducedMotion> {
  const replay = await openReplay(page, scenario, theme, { reducedMotion: true });
  const violations = new Set<string>();
  const sample = async () => (await page.evaluate(motionProbe)).running.forEach((r) => violations.add(r));
  await sample();
  while (replay.index + 1 < replay.states.length) {
    await replay.advance();
    await sample();
  }
  const before = await page.evaluate(canvasPixels);
  if (before.length > 0) {
    await page.waitForTimeout(500);
    const after = await page.evaluate(canvasPixels);
    before.forEach((b, i) => {
      if (b !== after[i]) violations.add(`canvas ${i} drew new pixels within 500 ms`);
    });
  }
  await page.evaluate(() => window.scrollTo(0, 0));
  await settle(page);
  return { violations: [...violations], flow: await page.evaluate(flowProbe) };
}

async function reducedMotion(browser: Browser, scenario: Scenario, viewport: Viewport, theme: Theme): Promise<ReducedMotion> {
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, timezoneId: 'UTC', locale: 'en-GB', reducedMotion: 'reduce' });
  try {
    return await reducedMotionReplay(await context.newPage(), scenario, theme);
  } finally {
    await context.close();
  }
}

// The always-visible elements at each of `viewports`, at every state of one replay: the page is
// resized to each viewport in turn after a state arrives, scrolled to the top and to the bottom.
export async function measureHeaders(browser: Browser, scenario: Scenario, theme: Theme, viewports: Viewport[]): Promise<HeaderMeasurement> {
  const first = viewports[0]!;
  const context = await browser.newContext({ viewport: { width: first.width, height: first.height }, timezoneId: 'UTC', locale: 'en-GB' });
  const page = await context.newPage();
  try {
    const replay = await openReplay(page, scenario, theme);
    await markAlways(page, replay);
    const seen = new Map<string, Map<string, number>>(viewports.map((v) => [v.name, new Map()]));
    const scrollers = new Map<string, Map<string, string>>(viewports.map((v) => [v.name, new Map()]));
    const probe = async () => {
      for (const v of viewports) {
        await page.setViewportSize({ width: v.width, height: v.height });
        await settle(page);
        keepHeader(seen.get(v.name)!, await page.evaluate<HeaderItem[]>(HEADER_PROBE));
        const into = scrollers.get(v.name)!;
        for (const entry of (await page.evaluate(layoutProbe)).scrollers) {
          const [key, label = entry] = entry.split(' | ');
          if (!into.has(key!)) into.set(key!, label);
        }
      }
    };
    await probe();
    while (replay.index + 1 < replay.states.length) {
      await replay.advance();
      await probe();
    }
    for (const action of scenario.actions ?? []) {
      await replay.act(action);
      await page.mouse.move(0, 0);
      await probe();
    }
    return {
      run: runId(),
      scenario: scenario.name,
      theme,
      viewports: Object.fromEntries([...seen.entries()].map(([k, m]) => [k, headerList(m)])),
      scrollers: Object.fromEntries([...scrollers.entries()].map(([k, m]) => [k, [...m.entries()].map(([key, label]) => ({ key, label }))])),
    };
  } finally {
    await context.close();
  }
}

export function writeMeasurement(dir: string, m: Measurement): string {
  mkdirSync(dir, { recursive: true });
  const file = join(dir, `${m.scenario}__${m.viewport.name}__${m.theme}.json`);
  writeFileSync(file, `${JSON.stringify(m, null, 2)}\n`);
  return file;
}

// Header measurements go to a directory of their own, so readMeasurements never reads them.
export function headersDir(dir: string): string {
  return join(dir, 'headers');
}

export function writeHeaderMeasurement(dir: string, m: HeaderMeasurement): string {
  const into = headersDir(dir);
  mkdirSync(into, { recursive: true });
  const file = join(into, `${m.scenario}__${m.theme}.json`);
  writeFileSync(file, `${JSON.stringify(m, null, 2)}\n`);
  return file;
}
