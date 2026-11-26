// Loads the built page on a fake origin and replays a scenario through the page's own feed: the
// first state answers `dashboard/snapshot`, and each later state answers one `dashboard/events`
// connection. Every other request is aborted, so nothing leaves the browser.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import type { Locator, Page, Route } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import type { Action, Scenario } from '../ux/scenarios/index.ts';
import type { Theme } from './profiles.ts';

export const ORIGIN = 'http://gateway.test';
export const PAGE_URL = `${ORIGIN}/dashboard`;
const BUILT = fileURLToPath(new URL('../../crates/llm-gateway/src/dashboard.html', import.meta.url));

let html: string | null = null;
function builtPage(): string {
  html ??= readFileSync(BUILT, 'utf8');
  return html;
}

// Counts layout shifts without recent input, reset by the harness after each state change.
const OBSERVER = `
  window.__ux = { shift: 0 };
  try {
    new PerformanceObserver((list) => {
      for (const e of list.getEntries()) if (!e.hadRecentInput) window.__ux.shift += e.value;
    }).observe({ type: 'layout-shift', buffered: true });
  } catch (e) {}
`;

export interface Replay {
  page: Page;
  states: Snapshot[];
  index: number;
  blocked: string[];
  toggle: Locator | null;
  themeApplied: boolean;
  advance(): Promise<void>;
  act(action: Action): Promise<void>;
  takeShift(): Promise<number>;
}

function sse(s: Snapshot): string {
  return `retry: 20\nevent: snapshot\ndata: ${JSON.stringify(s)}\n\n`;
}

export async function settle(page: Page): Promise<void> {
  await page.evaluate(
    () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(r, 0)))),
  );
}

export async function openReplay(page: Page, scenario: Scenario, theme: Theme, opts: { reducedMotion?: boolean } = {}): Promise<Replay> {
  const states = scenario.states(Date.now());
  const blocked: string[] = [];
  let index = 0;
  let held: Route | null = null;
  let onEventsRequest: (() => void) | null = null;

  await page.emulateMedia({ colorScheme: theme, reducedMotion: opts.reducedMotion ? 'reduce' : 'no-preference' });
  await page.addInitScript(OBSERVER);
  await page.route('**/*', async (route) => {
    const url = route.request().url();
    if (url === PAGE_URL) {
      await route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: builtPage() });
    } else if (url === `${PAGE_URL}/snapshot`) {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(states[index]) });
    } else if (url === `${PAGE_URL}/events`) {
      held = route;
      const notify = onEventsRequest;
      onEventsRequest = null;
      notify?.();
    } else {
      blocked.push(url);
      await route.abort('blockedbyclient');
    }
  });

  const firstEvents = new Promise<void>((r) => (onEventsRequest = r));
  await page.goto(PAGE_URL);
  await page.locator('main:not(.waiting)').first().waitFor();
  await firstEvents;
  await settle(page);

  const replay: Replay = {
    page,
    states,
    get index() {
      return index;
    },
    blocked,
    toggle: null,
    themeApplied: false,
    async advance() {
      if (index + 1 >= states.length) throw new Error('no state left');
      index += 1;
      const route = held;
      held = null;
      if (!route) throw new Error('the page holds no events connection');
      const next = new Promise<void>((r) => (onEventsRequest = r));
      await route.fulfill({ status: 200, contentType: 'text/event-stream', headers: { 'cache-control': 'no-store' }, body: sse(states[index]!) });
      // The page reconnects only after it has read the whole stream, so the state is applied.
      await next;
      await settle(page);
    },
    async act(action: Action) {
      if (action === 'expand-first-attempts') {
        await page.locator('button[aria-expanded="false"]').first().click();
      } else {
        await page.getByRole('button', { name: /^Show all/ }).click();
      }
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
  };
  replay.toggle = await findThemeToggle(page);
  if (replay.toggle) await setTheme(page, replay.toggle, theme);
  replay.themeApplied = (await effectiveTheme(page)) === theme;
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

async function effectiveTheme(page: Page): Promise<Theme> {
  const dark = await page.evaluate(() => {
    const parse = (c: string) => (c.match(/[\d.]+/g) ?? []).slice(0, 3).map(Number);
    const lum = (c: string) => {
      const [r = 255, g = 255, b = 255] = parse(c);
      return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
    };
    let bg = getComputedStyle(document.body).backgroundColor;
    if (bg === 'rgba(0, 0, 0, 0)' || bg === 'transparent') bg = getComputedStyle(document.documentElement).backgroundColor;
    if (bg === 'rgba(0, 0, 0, 0)' || bg === 'transparent') return false;
    return lum(bg) < 0.4;
  });
  return dark ? 'dark' : 'light';
}

async function setTheme(page: Page, toggle: Locator, theme: Theme): Promise<void> {
  const want = new RegExp(`^\\s*${theme}\\b`, 'i');
  const role = await toggle.getAttribute('role');
  const tag = await toggle.evaluate((el) => el.tagName.toLowerCase());
  if (role === 'radiogroup' || role === 'group') {
    const radio = toggle.getByRole('radio', { name: want });
    if ((await radio.count()) > 0) await radio.first().check();
    else await toggle.getByRole('button', { name: want }).first().click();
  } else if (tag === 'select' || role === 'combobox') {
    const options = await toggle.locator('option').allTextContents();
    const label = options.find((o) => want.test(o));
    if (label) await toggle.selectOption({ label });
  } else {
    const exact = page.getByRole('button', { name: want });
    if ((await exact.count()) > 0) {
      await exact.first().click();
    } else {
      for (let i = 0; i < 3 && (await effectiveTheme(page)) !== theme; i++) await toggle.click();
    }
  }
  await settle(page);
  await page.mouse.move(0, 0);
}
