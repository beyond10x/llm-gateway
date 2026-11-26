// The regression assertions of change set R-06 (UXF-010, 016, 011, 014, 019), each run against the
// built page through the hermetic replay.
import { expect, test, type Browser, type Page } from '@playwright/test';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay } from './harness.ts';
import { VIEWPORTS, type Theme, type Viewport } from './profiles.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;
const viewport = (name: string): Viewport => VIEWPORTS.find((v) => v.name === name)!;

async function pageAt(browser: Browser, v: Viewport): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: v.width, height: v.height }, timezoneId: 'UTC', locale: 'en-GB' });
  return context.newPage();
}

async function opened(browser: Browser, name: string, v = viewport('1366x768'), theme: Theme = 'light'): Promise<Page> {
  const page = await pageAt(browser, v);
  await openReplay(page, scenario(name), theme);
  return page;
}

test.describe('UXF-010', () => {
  test('a limit block is named by what it limits, its header lines read as labels, and the attempts button says how many attempts', async ({ browser }) => {
    const page = await opened(browser, 'busy');
    try {
      const headings = (await page.locator('.card h3').allTextContents()).map((t) => t.replace(/\s+/g, ' ').trim());
      expect(headings.length, 'two limit blocks in busy').toBe(2);
      for (const h of headings) {
        expect(h, 'named by what it limits').toMatch(/^Subscription limits · \S/);
        expect(h, 'no credential hash in the heading').not.toMatch(/\b[0-9a-f]{8}\b/);
      }
      expect(headings.some((h) => h.startsWith('Subscription limits · unified'))).toBe(true);
      expect(await page.locator('.card h3 [title*="3fa9c1d2"]').count(), 'the hash is kept in a title').toBe(1);

      const names = page.locator('.card dl.headers dt');
      expect(await names.count()).toBeGreaterThan(0);
      for (const dt of await names.all()) {
        const text = ((await dt.textContent()) ?? '').trim();
        expect(text, 'a readable label').not.toMatch(/anthropic-ratelimit|[a-z]-[a-z]/);
        expect(await dt.getAttribute('title'), `the raw header name of "${text}" in its title`).toMatch(/^anthropic-ratelimit-/);
      }
      expect(await names.allTextContents()).toContain('Requests limit');
    } finally {
      await page.context().close();
    }

    const errors = await opened(browser, 'errors');
    try {
      const button = errors.getByRole('button', { name: '2 attempts', exact: true });
      expect(await button.count(), 'the expander is named "2 attempts"').toBeGreaterThan(0);
      await expect(button.first()).toHaveAttribute('aria-expanded', 'false');
      expect(((await button.first().textContent()) ?? '').replace(/\s+/g, ' ')).toContain('2 attempts');
    } finally {
      await errors.context().close();
    }
  });
});
