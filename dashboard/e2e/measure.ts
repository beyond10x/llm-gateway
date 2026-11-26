// One measurement of one scenario at one viewport in one theme: replays every state, probes the
// layout and runs axe-core after each, measures the token-flow canvas, walks the page by keyboard
// and replays again with reduced motion.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import AxeBuilder from '@axe-core/playwright';
import type { Browser, Page } from '@playwright/test';
import type { Scenario } from '../ux/scenarios/index.ts';
import { openReplay, settle, type Replay } from './harness.ts';
import { axeRules, keyFontDeficit, mergeAxe, type AxeFinding, type Metrics } from './metrics.ts';
import { runId } from './metrics-dir.ts';
import { ALWAYS_ATTR, canvasPixels, flowProbe, focusStep, layoutProbe, markCandidates, motionProbe, type FlowProbe, type HeaderItem } from './probe.ts';
import { AXE_TAGS, type Theme, type Viewport } from './profiles.ts';

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
  // null when the page shows no key data.
  key_font_min_px: number | null;
  key_font_at: string;
  header_min_visible_ratio: number;
  shifts: number[];
  // The connection status text at every state.
  status: string[];
  snapshot_fetches: number;
  blocked_requests: number;
  blocked: string[];
  // The token-flow canvas at the last state, scrolled to the top.
  flow: FlowProbe;
  tools: { chromium: string; axe_core: string };
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
  };
}

const round3 = (x: number) => Math.round(x * 1000) / 1000;

async function markAlways(page: Page, replay: Replay): Promise<void> {
  const h1 = page.locator('h1').first();
  if ((await h1.count()) > 0) await h1.evaluate((el, a) => el.setAttribute(a, 'h1'), ALWAYS_ATTR);
  const status = page.locator('header [role="status"], [role="status"]').first();
  if ((await status.count()) > 0) await status.evaluate((el, a) => el.setAttribute(a, 'connection status'), ALWAYS_ATTR);
  if (replay.toggle) await replay.toggle.evaluate((el, a) => el.setAttribute(a, 'theme toggle'), ALWAYS_ATTR);
}

async function axe(page: Page): Promise<{ findings: AxeFinding[]; version: string }> {
  const result = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
  const findings = result.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => {
      const targets = [...new Set(v.nodes.map((n) => n.target.join(' ')))].sort();
      return { id: v.id, impact: v.impact ?? '', help: v.help, nodes: targets.length, targets };
    });
  return { findings, version: result.testEngine.version };
}

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

export async function measure(browser: Browser, scenario: Scenario, viewport: Viewport, theme: Theme, shotPath?: string): Promise<Measurement> {
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, timezoneId: 'UTC', locale: 'en-GB' });
  const page = await context.newPage();
  try {
    const replay = await openReplay(page, scenario, theme);
    await markAlways(page, replay);

    // Keyed by element selector, so an element counts once over every state.
    const overflowing = new Map<string, string>();
    const scrollers = new Map<string, string>();
    const truncated = new Map<string, string>();
    const keep = (into: Map<string, string>) => (entry: string) => {
      const [key, label = entry] = entry.split(' | ');
      if (!into.has(key!)) into.set(key!, label);
    };
    const header = new Map<string, number>();
    let overflowPx = 0;
    let keyFont = Infinity;
    let keyFontAt = '';
    const shifts: number[] = [];
    const status: string[] = [];
    const axeFindings = new Map<string, AxeFinding>();
    let axeScans = 0;
    let axeVersion = '';

    const probe = async () => {
      for (const scrollTo of ['top', 'bottom'] as const) {
        await page.evaluate((where) => window.scrollTo(0, where === 'top' ? 0 : document.documentElement.scrollHeight), scrollTo);
        await settle(page);
        const p = await page.evaluate(layoutProbe);
        overflowPx = Math.max(overflowPx, p.overflow_x_px);
        p.overflowing.forEach(keep(overflowing));
        p.scrollers.forEach(keep(scrollers));
        p.truncated.forEach(keep(truncated));
        for (const h of p.header) header.set(h.name, Math.min(header.get(h.name) ?? 1, h.visible_ratio));
        if (p.key_font_min_px > 0 && p.key_font_min_px < keyFont) {
          keyFont = p.key_font_min_px;
          keyFontAt = p.key_font_at;
        }
      }
      await page.evaluate(() => window.scrollTo(0, 0));
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
    if (shotPath) {
      await page.waitForTimeout(750);
      mkdirSync(join(shotPath, '..'), { recursive: true });
      await page.screenshot({ path: shotPath, animations: 'allow' });
    }
    for (const action of scenario.actions ?? []) {
      await replay.act(action);
      await page.mouse.move(0, 0);
      await probe();
      await scan();
    }
    const walk = await keyboardWalk(page);
    const motion = await reducedMotion(browser, scenario, viewport, theme);

    const headerItems = [...header.entries()].map(([name, visible_ratio]) => ({ name, visible_ratio: round3(visible_ratio) }));
    const headerMin = headerItems.length ? Math.min(...headerItems.map((h) => h.visible_ratio)) : 0;
    const axeList = [...axeFindings.values()].sort((a, b) => a.id.localeCompare(b.id));
    const keyFontMin = Number.isFinite(keyFont) ? round3(keyFont) : null;
    const items = (m: Map<string, string>): Item[] => [...m.entries()].map(([key, label]) => ({ key, label }));
    const metrics: Metrics = {
      'axe.serious_critical': axeList.reduce((a, f) => a + f.nodes, 0),
      'overflow.x_px': Math.round(overflowPx),
      'overflow.elements': overflowing.size,
      'overflow.scrollers_unreachable': walk.scrollersUnreachable.length,
      'header.hidden_ratio': round3(1 - headerMin),
      'keyboard.unreachable': walk.unreachable.length,
      'keyboard.focus_invisible': walk.invisible.length,
      'key_font.deficit_px': keyFontDeficit(keyFontMin),
      'layout_shift.per_state_max': shifts.length ? Math.max(...shifts) : 0,
      'reduced_motion.violations': motion.length,
      'truncation.without_full_value': truncated.size,
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
      key_font_min_px: keyFontMin,
      key_font_at: keyFontAt,
      header_min_visible_ratio: headerMin,
      shifts,
      status,
      snapshot_fetches: replay.snapshotFetches,
      blocked_requests: replay.blocked.length,
      blocked: [...replay.blocked],
      flow,
      tools: { chromium: browser.version(), axe_core: axeVersion },
      details: {
        axe: axeList,
        overflowing: items(overflowing),
        scrollers: items(scrollers),
        scrollers_unreachable: walk.scrollersUnreachable,
        truncated: items(truncated),
        header: headerItems,
        unreachable: walk.unreachable,
        focus_invisible: walk.invisible,
        reduced_motion: motion,
      },
    };
  } finally {
    await context.close();
  }
}

// With prefers-reduced-motion: reduce, no CSS animation or transition runs while the replay steps,
// and every canvas keeps the same pixels over 500 ms.
async function reducedMotion(browser: Browser, scenario: Scenario, viewport: Viewport, theme: Theme): Promise<string[]> {
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, timezoneId: 'UTC', locale: 'en-GB', reducedMotion: 'reduce' });
  const page = await context.newPage();
  try {
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
    return [...violations];
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
