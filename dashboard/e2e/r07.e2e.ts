// The regression assertions of change set R-07, each run against the
// built page through the hermetic replay.
import { expect, test, type Browser, type Page } from '@playwright/test';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, type Replay } from './harness.ts';
import { HEADING_FONT_PX, VIEWPORTS, type Theme, type Viewport } from './profiles.ts';
import { stripLines } from './r06-problems.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;
const viewport = (name: string): Viewport => VIEWPORTS.find((v) => v.name === name)!;

async function pageAt(browser: Browser, v: Viewport): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: v.width, height: v.height }, timezoneId: 'UTC', locale: 'en-GB' });
  return context.newPage();
}

async function replayed(browser: Browser, name: string, v = viewport('1366x768'), theme: Theme = 'light'): Promise<Replay> {
  return openReplay(await pageAt(browser, v), scenario(name), theme);
}

async function eachState(replay: Replay, check: (index: number) => Promise<void>): Promise<void> {
  await check(replay.index);
  while (replay.index + 1 < replay.states.length) {
    await replay.advance();
    await check(replay.index);
  }
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

// The failed-calls line of the Problems strip and the Routes table count the same thing: calls of
// the last 5 minutes, failed by one definition. The line names every route the table shows as
// failing, with the table's numbers and the status and kind of error of its failed calls.
interface RouteRow {
  route: string;
  recent: string;
}

async function routeRows(page: Page): Promise<RouteRow[]> {
  return page.locator('table.routes tbody tr').evaluateAll((trs) =>
    trs.map((tr) => ({
      route: ((tr.querySelector('th') as HTMLElement | null)?.innerText ?? '').split('\n')[0]!.trim(),
      recent: ((tr.querySelector('td.col-recent') as HTMLElement | null)?.innerText ?? '').replace(/\s+/g, ' ').trim(),
    })),
  );
}

test.describe('UXF-026', () => {
  for (const name of ['busy', 'errors']) {
    test(`the failed-calls line counts what Routes counts for the same 5 minutes, names every failing route with its cause, and links to Routes — ${name}`, async ({ browser }) => {
      const replay = await replayed(browser, name);
      const { page } = replay;
      const truth = scenario(name).truth!.failing_route!;
      let lines = 0;
      try {
        await eachState(replay, async (i) => {
          const at = `${name} state ${i}`;
          const line = (await stripLines(page)).find((l) => /^Failed calls\b/.test(l.text));
          const rows = await routeRows(page);
          expect(rows.length, `${at}: Routes rows`).toBeGreaterThan(0);
          const failing = rows.filter((r) => /^[1-9]\d* of \d+$/.test(r.recent));
          if (!line) {
            expect(failing, `${at}: no line, so no route failed in the window`).toEqual([]);
            return;
          }
          lines += 1;
          expect(line.href, `${at}: the line links to Routes`).toBe('#h-routes');
          expect(line.text, `${at}: the line says its window`).toMatch(/^Failed calls in the last 5 min: /);
          const segments = [...line.text.replace(/^[^:]*: /, '').matchAll(/(?:^|; )(\S+) (\d+) of (\d+) failed(?:, ([^;]+))?/g)].map((m) => ({ route: m[1]!, of: `${m[2]} of ${m[3]}`, cause: m[4] ?? '' }));
          expect(segments.map((s) => s.route).sort(), `${at}: "${line.text}" names every failing route of the table`).toEqual(failing.map((r) => r.route).sort());
          for (const s of segments) {
            expect(s.of, `${at}: ${s.route} on the line and in Routes`).toBe(rows.find((r) => r.route === s.route)?.recent);
            expect(s.cause, `${at}: ${s.route} names its status and the kind of error`).toMatch(/\b[1-5]\d\d [a-z]/);
          }
          const cause = segments.find((s) => s.route === truth.route)?.cause ?? '';
          expect(truth.cause.some((c) => new RegExp(c, 'i').test(cause)), `${at}: "${cause}" says why ${truth.route} fails`).toBe(true);
        });
        expect(lines, `${name}: a failed-calls line in some state`).toBeGreaterThan(0);
      } finally {
        await page.context().close();
      }
    });
  }
});
