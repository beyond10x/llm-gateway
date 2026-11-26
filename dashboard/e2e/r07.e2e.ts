// The regression assertions of change set R-07, each run against the
// built page through the hermetic replay.
import { expect, test, type Browser, type Page } from '@playwright/test';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, type Replay } from './harness.ts';
import { HEADING_FONT_PX, VIEWPORTS, type Theme, type Viewport } from './profiles.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;
const viewport = (name: string): Viewport => VIEWPORTS.find((v) => v.name === name)!;

async function pageAt(browser: Browser, v: Viewport): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: v.width, height: v.height }, timezoneId: 'UTC', locale: 'en-GB' });
  return context.newPage();
}

async function replayed(browser: Browser, name: string, v = viewport('1366x768'), theme: Theme = 'light'): Promise<Replay> {
  return openReplay(await pageAt(browser, v), scenario(name), theme);
}

// A heading is read at the size of its own text and of every text inside it (a card title holds
// the row's key in a span) but the secondary text the key-data probe floors at 12 px (a count
// badge, a muted note), and an h2 against every table cell of the section it heads.
test.describe('UXF-021', () => {
  test('section and card headings are 14 px or more and larger than the cells under them', async ({ browser }) => {
    for (const name of ['busy', 'errors']) {
      const { page } = await replayed(browser, name);
      try {
        const found = await page.evaluate(() => {
          const px = (el: Element) => parseFloat(getComputedStyle(el).fontSize);
          const hasText = (el: Element) => [...el.childNodes].some((n) => n.nodeType === Node.TEXT_NODE && (n.textContent ?? '').trim());
          const headings = [...document.querySelectorAll('main h2, main h3')].filter((h) => h.getBoundingClientRect().height > 0);
          const sizes = headings.flatMap((h) => [h, ...h.querySelectorAll('*')].filter((el) => hasText(el) && !el.closest('.badge, .small')).map((el) => ({ heading: (h.textContent ?? '').trim().slice(0, 40), tag: el.tagName.toLowerCase(), px: px(el) })));
          const over = [...document.querySelectorAll('main h2')].map((h) => {
            const section = h.closest('section');
            const cells = section ? [...section.querySelectorAll('tbody td, tbody th')].filter(hasText).map(px) : [];
            return { heading: (h.textContent ?? '').trim().slice(0, 40), px: px(h), cell: cells.length ? Math.max(...cells) : 0 };
          });
          return { sizes, over, cards: document.querySelectorAll('main .card h3').length };
        });
        expect(found.sizes.length, `${name}: headings on the page`).toBeGreaterThan(5);
        expect(found.cards, `${name}: limit cards with a title`).toBeGreaterThan(0);
        expect(found.sizes.filter((s) => s.px < HEADING_FONT_PX), `${name}: headings under ${HEADING_FONT_PX} px`).toEqual([]);
        expect(found.over.filter((h) => !(h.px > h.cell)), `${name}: an h2 not larger than a cell under it`).toEqual([]);
      } finally {
        await page.context().close();
      }
    }
  });
});
