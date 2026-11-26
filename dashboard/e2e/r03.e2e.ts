// The regression assertions of change set R-03 (UXF-002, 003, 004, 007, 008), each run against the
// built page through the hermetic replay.
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Browser, type Page } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, settle } from './harness.ts';
import { VIEWPORTS } from './profiles.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;
const viewport = (name: string) => {
  const v = VIEWPORTS.find((x) => x.name === name)!;
  return { width: v.width, height: v.height };
};

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
