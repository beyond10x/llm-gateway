// The regression assertions of change set R-06 (UXF-010, 016, 011, 014, 019), each run against the
// built page through the hermetic replay.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { parse as parseYaml } from 'yaml';
import { FONT_PX, FlowModel } from '../src/flow.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, settle } from './harness.ts';
import { KEY_FONT_PX, VIEWPORTS, type Theme, type Viewport } from './profiles.ts';

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

// The text of each matching element as a person reads it out or copies it: textContent with runs
// of whitespace as one space.
async function texts(page: Page, selector: string): Promise<string[]> {
  return (await page.locator(selector).allTextContents()).map((t) => t.replace(/\s+/g, ' ').trim());
}

test.describe('UXF-016', () => {
  for (const name of ['idle', 'busy', 'errors']) {
    test(`a second line or note is set apart from the words before it by a space — ${name}`, async ({ browser }) => {
      const page = await opened(browser, name);
      try {
        const windows = (await texts(page, '.tile .sub')).filter((t) => t.includes('last 60 s'));
        expect(windows.length, 'the two token tiles name their window').toBe(2);
        for (const t of windows) expect(t, 'the window is a word of its own').toMatch(/\S last 60 s$/);

        if (name !== 'idle') {
          const sessions = await texts(page, 'table.sessions tbody th');
          expect(sessions.length).toBeGreaterThan(0);
          for (const t of sessions) expect(t, 'the label and the id are two words').toMatch(/^\S+ [0-9a-f]{8,}$/);
          const states = (await texts(page, 'table.inflight td.col-state')).filter((t) => t.includes('attempt'));
          expect(states.length, 'a call in flight on its second attempt').toBeGreaterThan(0);
          for (const t of states) expect(t, 'the attempt is a word of its own').toMatch(/\S attempt \d+$/);
        }

        const main = (await page.locator('main').textContent()) ?? '';
        expect(main, 'no run-together window').not.toMatch(/tok\/slast|–last/);
        expect(main, 'no run-together attempt').not.toMatch(/[a-z]attempt \d/);
      } finally {
        await page.context().close();
      }
    });
  }
});

// The document's one table of display words for state values (label_maps.state).
const DOCUMENT = parseYaml(readFileSync(fileURLToPath(new URL('../ui/llm-gateway.ui.yaml', import.meta.url)), 'utf8')) as { label_maps?: Record<string, Record<string, string>> };
const STATE_WORDS: Record<string, string> = DOCUMENT.label_maps?.state ?? {};

interface Shown {
  where: string;
  text: string;
  raw: string | null;
}

// Every state value the page shows, with the raw value its title holds.
async function statesShown(page: Page): Promise<Shown[]> {
  return page.evaluate(() => {
    const out: { where: string; text: string; raw: string | null }[] = [];
    const norm = (t: string | null) => (t ?? '').replace(/\s+/g, ' ').trim();
    const take = (where: string, el: Element) => {
      const titled = el.hasAttribute('title') ? el : el.querySelector('[title]');
      out.push({ where, text: norm(el.textContent), raw: titled?.getAttribute('title') ?? null });
    };
    for (const [where, sel] of [
      ['in flight', 'table.inflight td.col-state .pill'],
      ['pod state', 'table.pods td.col-state .pill'],
      ['vLLM ready', 'table.pods td.col-vllm_ready'],
      ['call status', 'table.calls td.col-status .pill'],
      ['attempt status', 'table.mini td.col-status .pill'],
    ] as const) {
      for (const el of document.querySelectorAll(sel)) take(where, el);
    }
    for (const tile of document.querySelectorAll('.card .tile')) {
      const label = norm(tile.querySelector('.label')?.textContent ?? '');
      const value = tile.querySelector('.value');
      if (value && ['Status', 'Overage', 'Overage reason', 'Representative claim'].includes(label)) take(`limit ${label}`, value);
    }
    return out;
  });
}

test.describe('UXF-011', () => {
  test('the state words come from one table: every state badge and limit status is spelled by it, with the raw value in its title', async ({ browser }) => {
    expect(Object.keys(STATE_WORDS).length, 'label_maps.state in the document').toBeGreaterThan(0);
    const spellings = Object.values(STATE_WORDS);
    for (const w of spellings) expect(w, 'a display word has no underscore and is not in capitals').toMatch(/^(?!.*_)(?!.*\b[A-Z]{2,}\b).+$/);
    const seen = new Map<string, Set<string>>();
    for (const name of ['busy', 'errors']) {
      const page = await opened(browser, name);
      try {
        await page.locator('button[aria-expanded="false"]').first().click();
        const shown = await statesShown(page);
        const places = new Set(shown.map((s) => s.where));
        for (const p of ['in flight', 'call status', 'attempt status', 'limit Status']) expect(places, `${name} shows ${p}`).toContain(p);
        if (name === 'busy') for (const p of ['pod state', 'vLLM ready']) expect(places, `${name} shows ${p}`).toContain(p);
        for (const s of shown) {
          const at = `${name}, ${s.where}: "${s.text}"`;
          expect(spellings, `${at} is a word of the table`).toContain(s.text);
          expect(s.raw, `${at} keeps its raw value in a title`).toBeTruthy();
          (seen.get(s.raw!) ?? seen.set(s.raw!, new Set()).get(s.raw!)!).add(s.text);
        }
      } finally {
        await page.context().close();
      }
    }
    const twice = [...seen].filter(([, words]) => words.size > 1).map(([raw, words]) => `${raw}: ${[...words].join(' / ')}`);
    expect(twice, 'one raw value, one spelling').toEqual([]);
  });
});

// The token flow is a canvas, so its labels are read from the layout the canvas draws (FlowModel,
// the code FlowCanvas.vue paints with) at the size the page gives the canvas's box.
test.describe('UXF-014', () => {
  test('the token flow draws its labels at the key-data size', () => {
    expect(FONT_PX).toBeGreaterThanOrEqual(KEY_FONT_PX);
  });

  for (const name of ['busy', 'token-flow', 'errors']) {
    test(`every route name in the token flow is drawn whole at every viewport, 390x844 included — ${name}`, async ({ browser }) => {
      const page = await opened(browser, name, viewport('390x844'));
      try {
        const states = scenario(name).states(Date.now());
        for (const v of VIEWPORTS) {
          await page.setViewportSize({ width: v.width, height: v.height });
          await settle(page);
          const box = await page.locator('.flow-box').evaluate((el) => ({ w: el.clientWidth, h: el.clientHeight }));
          const model = new FlowModel({ reduced: true });
          for (const [i, s] of states.entries()) {
            model.apply(s);
            const routes = [...model.layout(box.w, box.h).routes.values()];
            expect(routes.length, `${v.name} state ${i}: routes drawn`).toBe(model.routes.length);
            for (const r of routes) expect(r.text, `${v.name} state ${i}, a ${box.w} px wide canvas`).toBe(r.key);
          }
        }
      } finally {
        await page.context().close();
      }
    });
  }
});
