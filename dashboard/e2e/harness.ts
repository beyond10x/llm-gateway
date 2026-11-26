// Loads the built page on a fake origin and replays a scenario through the page's own feed, the
// way the gateway serves it: the first state answers the one `dashboard/snapshot` fetch, and an
// in-page stand-in for EventSource (installed before any page script) opens once and delivers one
// `snapshot` event per state. The connection status reads "live" as on the real stream and the
// page's 5 s poll never starts. Every other request is aborted, so nothing leaves the browser.
//
// Layout shift per state: the browser leaves out shifts within 500 ms of a click or key press
// (`hadRecentInput`). A state is therefore never pushed sooner than INPUT_QUIET_MS after the
// harness's last input, and its shift is read two animation frames after the push. The number
// does not depend on what ran before (an axe scan, a wait, a click).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { Locator, Page } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import type { Action, Scenario } from '../ux/scenarios/index.ts';
import { fontsLabel, pinFonts } from './fonts.ts';
import type { Theme } from './profiles.ts';

export const ORIGIN = 'http://gateway.test';
export const PAGE_URL = `${ORIGIN}/dashboard`;
const BUILT = fileURLToPath(new URL('../../crates/llm-gateway/src/dashboard.html', import.meta.url));

export const INPUT_QUIET_MS = 600;

// How a replay is measured, printed in every round report.
export const REPLAY_METHOD = [
  'Hermetic: every snapshot comes from `ux/scenarios/`; the page is the built `dashboard.html` on a fake origin; every other request is aborted (`blocked` in round.json).',
  'The first state answers the one `dashboard/snapshot` fetch; every state then arrives as one `snapshot` event on a stand-in event stream that opened once and stays open, as the gateway serves it, so the connection status reads "live" and the page never polls.',
  `Layout shift per state: the sum of layout-shift entries from the moment the state is pushed to two animation frames later. No state is pushed within ${INPUT_QUIET_MS} ms of a harness click or key press, since the browser leaves out shifts that soon after an input; the value does not depend on what ran before it (an axe scan, a wait).`,
  'axe-core scans every state of every replay and the page after every action; a rule counts the distinct nodes over all scans.',
  `Fonts: ${fontsLabel()}. The pinned fonts are committed under \`e2e/fonts/\` and installed in every replay before the page's first script, so every host measures the same text widths (e2e/fonts.ts); "host" measures with the fonts the host has installed and is not the gate.`,
  'The token-flow canvas is measured at the last state, scrolled to the top: `flow.visible_ratio` is the share of a 32 x 16 grid over its box that lies in the viewport with nothing painted over it; `flow.drawn` is whether it holds pixels other than its background.',
].join('\n');

let html: string | null = null;
function builtPage(): string {
  html ??= readFileSync(BUILT, 'utf8');
  return html;
}

// Counts layout shifts without recent input; the harness reads and resets it after each state.
const OBSERVER = `
  window.__ux = { shift: 0 };
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) if (!e.hadRecentInput) window.__ux.shift += e.value;
    }).observe({ type: 'layout-shift', buffered: true });
  } catch (e) {}
`;

// The page's event stream, driven by the harness through window.__uxStream.
const FAKE_STREAM = `
  (() => {
    const streams = [];
    class UxEventSource extends EventTarget {
      constructor(url) {
        super();
        this.url = String(url);
        this.readyState = 0;
        this.withCredentials = false;
        streams.push(this);
      }
      close() { this.readyState = 2; }
    }
    UxEventSource.CONNECTING = 0;
    UxEventSource.OPEN = 1;
    UxEventSource.CLOSED = 2;
    const last = () => {
      const s = streams[streams.length - 1];
      if (!s) throw new Error('the page opened no event stream');
      return s;
    };
    window.__uxStream = {
      count: () => streams.length,
      open() { const s = last(); s.readyState = 1; s.dispatchEvent(new Event('open')); },
      push(data) {
        const s = last();
        if (s.readyState !== 1) throw new Error('the event stream is not open');
        s.dispatchEvent(new MessageEvent('snapshot', { data }));
      },
      fail() { const s = last(); s.readyState = 0; s.dispatchEvent(new Event('error')); },
    };
    window.EventSource = UxEventSource;
  })();
`;

export interface Replay {
  page: Page;
  states: Snapshot[];
  index: number;
  blocked: string[];
  snapshotFetches: number;
  toggle: Locator | null;
  themeApplied: boolean;
  advance(): Promise<void>;
  act(action: Action): Promise<void>;
  click(target: Locator): Promise<void>;
  takeShift(): Promise<number>;
  status(): Promise<string>;
}

export async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(r, 0)))),
  );
}

async function ready(page: Page): Promise<void> {
  await page.locator('main:not(.waiting)').first().waitFor();
  await page.waitForFunction(() => (window as unknown as { __uxStream: { count(): number } }).__uxStream.count() > 0);
}

export async function openReplay(page: Page, scenario: Scenario, theme: Theme, opts: { reducedMotion?: boolean } = {}): Promise<Replay> {
  const states = scenario.states(Date.now());
  const blocked: string[] = [];
  let index = 0;
  let snapshotFetches = 0;
  let lastInput = 0;

  await page.emulateMedia({ colorScheme: theme, reducedMotion: opts.reducedMotion ? 'reduce' : 'no-preference' });
  await pinFonts(page);
  await page.addInitScript(OBSERVER);
  await page.addInitScript(FAKE_STREAM);
  await page.route('**/*', async (route) => {
    const url = route.request().url();
    if (url === PAGE_URL) {
      await route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: builtPage() });
    } else if (url === `${PAGE_URL}/snapshot`) {
      snapshotFetches += 1;
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(states[index]) });
    } else {
      blocked.push(url);
      await route.abort('blockedbyclient');
    }
  });

  const push = (s: Snapshot) =>
    page.evaluate((data) => (window as unknown as { __uxStream: { push(d: string): void } }).__uxStream.push(data), JSON.stringify(s));

  await page.goto(PAGE_URL);
  await ready(page);
  await page.evaluate(() => (window as unknown as { __uxStream: { open(): void } }).__uxStream.open());
  await push(states[0]!);
  await settle(page);

  const replay: Replay = {
    page,
    states,
    get index() {
      return index;
    },
    blocked,
    get snapshotFetches() {
      return snapshotFetches;
    },
    toggle: null,
    themeApplied: false,
    async advance() {
      if (index + 1 >= states.length) throw new Error('no state left');
      const wait = lastInput + INPUT_QUIET_MS - Date.now();
      if (wait > 0) await page.waitForTimeout(wait);
      index += 1;
      await push(states[index]!);
      await settle(page);
    },
    async act(action: Action) {
      if (action === 'expand-first-attempts') {
        await replay.click(page.locator('button[aria-expanded="false"]').first());
      } else {
        await replay.click(page.getByRole('button', { name: /^Show all/ }));
      }
    },
    async click(target: Locator) {
      await target.click();
      lastInput = Date.now();
      await settle(page);
    },
    async takeShift() {
      await settle(page);
      return page.evaluate(() => {
        const ux = (window as unknown as { __ux: { shift: number } }).__ux;
        const v = ux.shift;
        ux.shift = 0;
        return v;
      });
    },
    async status() {
      return ((await page.locator('header [role="status"]').first().textContent()) ?? '').trim();
    },
  };
  replay.toggle = await findThemeToggle(page);
  replay.themeApplied = (await palette(page)).shown === theme;
  await replay.takeShift();
  return replay;
}

// A theme control, found by role and name; null when the page has none.
export async function findThemeToggle(page: Page): Promise<Locator | null> {
  const name = /theme|dark|light|appearance|colou?r scheme/i;
  const candidates = [
    page.getByRole('radiogroup', { name }),
    page.getByRole('combobox', { name }),
    page.getByRole('switch', { name }),
    page.getByRole('button', { name }),
    page.getByRole('group', { name: /theme/i }),
  ];
  for (const c of candidates) {
    if ((await c.count()) > 0 && (await c.first().isVisible())) return c.first();
  }
  return null;
}

export interface Palette {
  bg: string;
  body: string;
  data_theme: string | null;
  shown: Theme;
}

// The palette the page applies: its --bg custom property, the body's computed background, and
// which theme that background reads as.
export async function palette(page: Page): Promise<Palette> {
  return page.evaluate(() => {
    const parse = (c: string) => (c.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
    const lum = (c: string) => {
      const [r = 255, g = 255, b = 255] = parse(c);
      return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
    };
    const root = document.documentElement;
    let body = getComputedStyle(document.body).backgroundColor;
    if (body === 'rgba(0, 0, 0, 0)' || body === 'transparent') body = getComputedStyle(root).backgroundColor;
    const transparent = body === 'rgba(0, 0, 0, 0)' || body === 'transparent';
    return {
      bg: getComputedStyle(root).getPropertyValue('--bg').trim(),
      body,
      data_theme: root.getAttribute('data-theme'),
      shown: !transparent && lum(body) < 0.4 ? ('dark' as const) : ('light' as const),
    };
  });
}

async function chooseTheme(replay: Replay, toggle: Locator, theme: Theme): Promise<void> {
  const page = replay.page;
  const want = new RegExp(`^\\s*${theme}\\b`, 'i');
  const role = await toggle.getAttribute('role');
  const tag = await toggle.evaluate((el) => el.tagName.toLowerCase());
  if (role === 'radiogroup' || role === 'group') {
    const radio = toggle.getByRole('radio', { name: want });
    await replay.click((await radio.count()) > 0 ? radio.first() : toggle.getByRole('button', { name: want }).first());
  } else if (tag === 'select' || role === 'combobox') {
    const options = await toggle.locator('option').allTextContents();
    const label = options.find((o) => want.test(o));
    if (label) await toggle.selectOption({ label });
  } else {
    const exact = page.getByRole('button', { name: want });
    if ((await exact.count()) > 0) {
      await replay.click(exact.first());
    } else {
      for (let i = 0; i < 3 && (await palette(page)).shown !== theme; i++) await replay.click(toggle);
    }
  }
  await settle(page);
  await page.mouse.move(0, 0);
}

// The theme toggle check: the page must show the other theme before the choice (so a toggle that
// does nothing cannot pass), the palette must change to `want` on the choice, and the choice must
// survive a reload. Returns what failed; empty when the toggle works.
export async function themeToggleProblems(replay: Replay, want: Theme): Promise<string[]> {
  const page = replay.page;
  const problems: string[] = [];
  const toggle = replay.toggle ?? (await findThemeToggle(page));
  if (!toggle) return ['no theme toggle'];
  const before = await palette(page);
  if (before.shown === want) problems.push(`the page shows ${want} before the choice: emulate the other colour scheme first`);
  await chooseTheme(replay, toggle, want);
  const after = await palette(page);
  if (after.bg === before.bg && after.body === before.body) problems.push(`choosing ${want} changed neither --bg nor the body background (${after.body})`);
  if (after.shown !== want) problems.push(`choosing ${want} shows ${after.shown}`);
  await page.reload();
  await ready(page);
  await settle(page);
  const reloaded = await palette(page);
  if (reloaded.shown !== want) problems.push(`after a reload the page shows ${reloaded.shown}, not the stored ${want}`);
  return problems;
}
