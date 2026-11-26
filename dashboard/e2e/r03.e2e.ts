// The regression assertions of change set R-03 (UXF-002, 003, 004, 007, 008), each run against the
// built page through the hermetic replay.
import AxeBuilder from '@axe-core/playwright';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay } from './harness.ts';
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
