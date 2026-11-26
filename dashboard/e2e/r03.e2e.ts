// The regression assertions of change set R-03 (UXF-002, 003, 004, 007, 008), each run against the
// built page through the hermetic replay.
import { mkdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Browser, type Page, type Route } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { PAGE_URL, openReplay, settle } from './harness.ts';
import { VIEWPORTS, type Theme } from './profiles.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;
const viewport = (name: string) => {
  const v = VIEWPORTS.find((x) => x.name === name)!;
  return { width: v.width, height: v.height };
};
const GATING = ['1366x768', '1280x720'];

async function freshPage(browser: Browser, vp: string): Promise<Page> {
  const context = await browser.newContext({ viewport: viewport(vp), timezoneId: 'UTC', locale: 'en-GB' });
  return context.newPage();
}

async function eachState(replay: { index: number; states: Snapshot[]; advance(): Promise<void> }, check: () => Promise<void>): Promise<void> {
  await check();
  while (replay.index + 1 < replay.states.length) {
    await replay.advance();
    await check();
  }
}

test.describe('UXF-002', () => {
  test('every table scroller is a named region that Tab reaches with a visible focus ring', async ({ page }) => {
    await page.setViewportSize(viewport('1280x720'));
    await openReplay(page, scenario('busy'), 'light');
    const regions = await page.evaluate(() =>
      [...document.querySelectorAll('main *')]
        .filter((el) => ['auto', 'scroll'].includes(getComputedStyle(el).overflowX))
        .map((el, i) => {
          el.setAttribute('data-r03-region', String(i));
          return { i, tabindex: el.getAttribute('tabindex'), role: el.getAttribute('role'), label: el.getAttribute('aria-label') ?? '' };
        }),
    );
    expect(regions.length).toBeGreaterThanOrEqual(5);
    for (const r of regions) {
      expect.soft(r.tabindex, `scroller ${r.i} tabindex`).toBe('0');
      expect.soft(r.role, `scroller ${r.i} role`).toBe('region');
      expect.soft(r.label, `scroller ${r.i} name`).toMatch(/table/i);
    }
    const reached = new Map<string, boolean>();
    for (let i = 0; i < 80 && reached.size < regions.length; i++) {
      await page.keyboard.press('Tab');
      const step = await page.evaluate(() => {
        const el = document.activeElement;
        const id = el?.getAttribute('data-r03-region') ?? null;
        if (!el || id === null) return null;
        const cs = getComputedStyle(el);
        return { id, ring: cs.outlineStyle !== 'none' && parseFloat(cs.outlineWidth) >= 2 };
      });
      if (step) reached.set(step.id, step.ring);
    }
    expect([...reached.keys()].sort()).toEqual(regions.map((r) => String(r.i)).sort());
    expect([...reached.values()].every(Boolean), 'focus ring on every region').toBe(true);
  });

  for (const vp of ['1280x720', '390x844']) {
    test(`axe scrollable-region-focusable finds nothing — ${vp}`, async ({ browser }) => {
      for (const name of ['busy', 'errors']) {
        const page = await freshPage(browser, vp);
        const replay = await openReplay(page, scenario(name), 'light');
        while (replay.index + 1 < replay.states.length) await replay.advance();
        const result = await new AxeBuilder({ page }).withRules(['scrollable-region-focusable']).analyze();
        expect(result.violations.flatMap((v) => v.nodes.map((n) => `${name}: ${n.target.join(' ')}`))).toEqual([]);
        await page.context().close();
      }
    });
  }
});

// Every element in a tile, a limit list and a limit heading lies inside that box.
function boxedOverflow(): string[] {
  const out: string[] = [];
  const name = (el: Element) => `${el.tagName.toLowerCase()}${el.className && typeof el.className === 'string' ? `.${el.className.split(' ').join('.')}` : ''} "${(el.textContent ?? '').trim().slice(0, 30)}"`;
  const within = (box: Element) => {
    const b = box.getBoundingClientRect();
    for (const el of box.querySelectorAll('*')) {
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      if (r.right > b.right + 1 || r.left < b.left - 1) out.push(`${name(el)} past ${name(box)}`);
    }
    for (const el of [box, ...box.querySelectorAll('*')]) {
      for (const n of el.childNodes) {
        if (n.nodeType !== Node.TEXT_NODE || !(n.textContent ?? '').trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(n);
        const r = range.getBoundingClientRect();
        if (r.right > b.right + 1 || r.left < b.left - 1) out.push(`text "${(n.textContent ?? '').trim().slice(0, 30)}" past ${name(box)}`);
      }
    }
  };
  for (const box of document.querySelectorAll('.tile, dl.headers, .limit h3')) within(box);
  return [...new Set(out)];
}

test.describe('UXF-003', () => {
  for (const name of ['busy', 'token-flow', 'errors']) {
    test(`tiles, limit headings and limit headers keep their contents inside at every viewport — ${name}`, async ({ page }) => {
      const replay = await openReplay(page, scenario(name), 'light');
      const found = new Set<string>();
      await eachState(replay, async () => {
        for (const v of VIEWPORTS) {
          await page.setViewportSize({ width: v.width, height: v.height });
          await settle(page);
          for (const o of await page.evaluate(boxedOverflow)) found.add(`${v.name}: ${o}`);
        }
      });
      expect([...found]).toEqual([]);
    });
  }
});

test.describe('UXF-007', () => {
  for (const name of SCENARIOS.map((s) => s.name)) {
    test(`every Routes column is in view without scrolling at 1366x768 and 1280x720 — ${name}`, async ({ page }) => {
      const replay = await openReplay(page, scenario(name), 'light');
      const cut = new Set<string>();
      await eachState(replay, async () => {
        for (const vp of GATING) {
          await page.setViewportSize(viewport(vp));
          await settle(page);
          const r = await page.evaluate(() => {
            const table = document.querySelector('#h-routes')?.closest('section')?.querySelector('table');
            const region = table?.parentElement;
            if (!table || !region) return { missing: true, cut: [] as string[], scrolls: false };
            const box = region.getBoundingClientRect();
            const cut = [...table.querySelectorAll('thead th')]
              .filter((th) => th.getBoundingClientRect().right > box.right + 1)
              .map((th) => (th.textContent ?? '').trim());
            return { missing: false, cut, scrolls: region.scrollWidth > region.clientWidth + 1 };
          });
          expect(r.missing, 'routes table').toBe(false);
          for (const c of r.cut) cut.add(`${vp}: ${c}`);
          if (r.scrolls) cut.add(`${vp}: the region scrolls`);
        }
      });
      expect([...cut]).toEqual([]);
    });
  }
});

// A replay of `name` on the page with ?flowdebug, so window.__flow() answers.
async function flowReplay(page: Page, name: string, theme: Theme): Promise<{ advanceAll(): Promise<void> }> {
  const states = scenario(name).states(Date.now());
  const html = readFileSync(fileURLToPath(new URL('../../crates/llm-gateway/src/dashboard.html', import.meta.url)), 'utf8');
  let index = 0;
  let held: Route | null = null;
  let waiting: (() => void) | null = null;
  await page.emulateMedia({ colorScheme: theme, reducedMotion: 'no-preference' });
  await page.addInitScript((t) => localStorage.setItem('llm-gateway.theme', t), theme);
  await page.route('**/*', async (route) => {
    const url = route.request().url();
    if (url === `${PAGE_URL}?flowdebug`) await route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', body: html });
    else if (url === `${PAGE_URL}/snapshot`) await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(states[index]) });
    else if (url === `${PAGE_URL}/events`) {
      held = route;
      const w = waiting;
      waiting = null;
      w?.();
    } else await route.abort('blockedbyclient');
  });
  const first = new Promise<void>((r) => (waiting = r));
  await page.goto(`${PAGE_URL}?flowdebug`);
  await page.locator('main:not(.waiting)').first().waitFor();
  await first;
  await settle(page);
  return {
    async advanceAll() {
      while (index + 1 < states.length) {
        index += 1;
        const route = held as Route | null;
        held = null;
        if (!route) throw new Error('no events connection held');
        const next = new Promise<void>((r) => (waiting = r));
        await route.fulfill({ status: 200, contentType: 'text/event-stream', body: `retry: 20\nevent: snapshot\ndata: ${JSON.stringify(states[index])}\n\n` });
        await next;
        await settle(page);
      }
    },
  };
}

interface FlowCountsSeen { strings: number; particles: number; drawn?: number; frames: number }

test.describe('UXF-008', () => {
  for (const theme of ['light', 'dark'] as const) {
    test(`the token flow has its own panel above Overview, in view and uncovered — ${theme}`, async ({ page }) => {
      await page.setViewportSize(viewport('1366x768'));
      const replay = await flowReplay(page, 'token-flow', theme);
      await replay.advanceAll();
      await page.waitForTimeout(400);

      const order = await page.evaluate(() => {
        const sections = [...document.querySelectorAll('main > section, main section')];
        const flow = [...document.querySelectorAll('main h2')].find((h) => (h.textContent ?? '').trim() === 'Token flow')?.closest('section') ?? null;
        const overview = document.querySelector('#h-kpi')?.closest('section') ?? null;
        return {
          flow: flow !== null,
          first: flow !== null && sections[0] === flow,
          above: !!flow && !!overview && flow.getBoundingClientRect().bottom <= overview.getBoundingClientRect().top,
          fixedCanvas: [...document.querySelectorAll('canvas')].filter((c) => getComputedStyle(c).position === 'fixed').length,
          caption: (flow?.querySelector('.flow-caption')?.textContent ?? '').replace(/\s+/g, ' ').trim(),
        };
      });
      expect(order.flow, 'a section headed "Token flow"').toBe(true);
      expect(order.first, 'the first section of main').toBe(true);
      expect(order.above, 'above Overview').toBe(true);
      expect(order.fixedCanvas, 'no fixed background canvas').toBe(0);
      expect(order.caption).toMatch(/^\d+ active sessions? → \d+ routes?\b/);

      for (const vp of GATING) {
        await page.setViewportSize(viewport(vp));
        await page.evaluate(() => window.scrollTo(0, 0));
        await settle(page);
        const probe = await page.evaluate(() => {
          const canvas = document.querySelector('main section canvas') as HTMLCanvasElement | null;
          if (!canvas) return null;
          const cs = getComputedStyle(canvas);
          const attrs = { hidden: canvas.getAttribute('aria-hidden'), tabindex: canvas.getAttribute('tabindex'), events: cs.pointerEvents };
          const r = canvas.getBoundingClientRect();
          const before = canvas.style.pointerEvents;
          canvas.style.pointerEvents = 'auto';
          const covered: string[] = [];
          for (let i = 0; i < 5; i++) {
            for (let j = 0; j < 5; j++) {
              const x = r.left + ((i + 0.5) * r.width) / 5;
              const y = r.top + ((j + 0.5) * r.height) / 5;
              const hit = document.elementFromPoint(x, y);
              if (hit !== canvas) covered.push(`${Math.round(x)},${Math.round(y)}: ${hit ? hit.tagName.toLowerCase() : 'nothing'}`);
            }
          }
          canvas.style.pointerEvents = before;
          return { attrs, box: { left: r.left, top: r.top, right: r.right, bottom: r.bottom, height: r.height }, vw: innerWidth, vh: innerHeight, covered };
        });
        expect(probe, `${vp}: a canvas in a section of main`).not.toBeNull();
        const p = probe!;
        expect(p.attrs).toEqual({ hidden: 'true', tabindex: null, events: 'none' });
        expect(p.box.left, vp).toBeGreaterThanOrEqual(0);
        expect(p.box.top, vp).toBeGreaterThanOrEqual(0);
        expect(p.box.right, vp).toBeLessThanOrEqual(p.vw);
        expect(p.box.bottom, vp).toBeLessThanOrEqual(p.vh);
        expect(p.box.height, vp).toBeGreaterThanOrEqual(120);
        expect(p.covered, vp).toEqual([]);
      }

      const counts = await page.evaluate(() => (window as unknown as { __flow?: () => FlowCountsSeen }).__flow?.() ?? null);
      expect(counts, 'window.__flow with ?flowdebug').not.toBeNull();
      expect(counts!.strings).toBeGreaterThanOrEqual(1);
      expect(counts!.drawn ?? 0, 'particles drawn').toBeGreaterThan(0);

      const shots = process.env.R03_SHOTS;
      if (shots) {
        await page.setViewportSize(viewport('1366x768'));
        await page.evaluate(() => window.scrollTo(0, 0));
        await settle(page);
        mkdirSync(shots, { recursive: true });
        await page.screenshot({ path: join(shots, `token-flow-1366x768-${theme}.png`) });
      }
    });
  }

  test('with no active session the panel says so in words', async ({ page }) => {
    await page.setViewportSize(viewport('1280x720'));
    await openReplay(page, scenario('idle'), 'light');
    const panel = page.locator('main section', { has: page.getByRole('heading', { name: 'Token flow' }) });
    await expect(panel).toHaveCount(1);
    await expect(panel.getByText('No active sessions')).toBeVisible();
  });
});

// Key values (token counts, rates, statuses, money) at 14 px or more; secondary text (second lines,
// badges, column heads, captions, tile labels) at 12 px or more.
function fontDeficits(): string[] {
  const out: string[] = [];
  const SECONDARY = '.small, .badge, caption, thead th, .tile .label, .stats dt';
  const describe = (el: Element) => `${el.tagName.toLowerCase()}${el.className && typeof el.className === 'string' ? `.${el.className.split(' ').join('.')}` : ''} "${(el.textContent ?? '').trim().slice(0, 30)}"`;
  for (const el of document.querySelectorAll('main *, header [role="status"]')) {
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || el.closest('.flow-panel, .skip')) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) continue;
    if (![...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? '').trim())) continue;
    const px = parseFloat(cs.fontSize);
    const key = el.closest('tbody td, tbody th, dd, .tile .value, .tile .sub, [role="status"]') !== null && el.closest(SECONDARY) === null;
    const secondary = el.closest(SECONDARY) !== null;
    if (key && px < 14 - 1e-6) out.push(`key ${px}px ${describe(el)}`);
    else if (secondary && px < 12 - 1e-6) out.push(`secondary ${px}px ${describe(el)}`);
  }
  return [...new Set(out)];
}

test.describe('UXF-004', () => {
  test('key values are 14 px or more and secondary text 12 px or more', async ({ browser }) => {
    const found = new Set<string>();
    for (const name of ['busy', 'errors']) {
      const page = await freshPage(browser, '1280x720');
      const replay = await openReplay(page, scenario(name), 'light');
      await eachState(replay, async () => {
        for (const f of await page.evaluate(fontDeficits)) found.add(`${name}: ${f}`);
      });
      for (const action of scenario(name).actions ?? []) {
        await replay.act(action);
        for (const f of await page.evaluate(fontDeficits)) found.add(`${name}: ${f}`);
      }
      await page.context().close();
    }
    expect([...found]).toEqual([]);
  });
});

