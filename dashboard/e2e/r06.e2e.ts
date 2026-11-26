// The regression assertions of change set R-06 (UXF-010, 016, 011, 014, 019), each run against the
// built page through the hermetic replay.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { parse as parseYaml } from 'yaml';
import { FONT_PX, FlowModel } from '../src/flow.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, settle, type Replay } from './harness.ts';
import { KEY_FONT_PX, THEMES, VIEWPORTS, type Theme, type Viewport } from './profiles.ts';
import { KINDS, dropConnection, kindsOf, stripLines, type Kind } from './r06-problems.ts';

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

// The strip at the top names each kind of problem the state holds, one line each, in words, fully on
// the first screen and uncovered, and holds nothing else; with no problem there is no strip.
async function stripProblems(page: Page, want: Kind[], v: Viewport, at: string): Promise<void> {
  const region = page.getByRole('region', { name: 'Problems', exact: true });
  if (want.length === 0) {
    expect(await region.count(), `${at}: no strip when nothing is wrong`).toBe(0);
    return;
  }
  expect(await region.count(), `${at}: a region named Problems`).toBe(1);
  const lines = await stripLines(page);
  expect(lines.map((l) => l.text).sort(), `${at}: one line per kind (${want.join(', ')})`).toHaveLength(want.length);
  for (const kind of want) {
    const k = KINDS[kind];
    const line = lines.find((l) => k.words.test(l.text));
    expect(line, `${at}: a line for ${kind} among ${JSON.stringify(lines.map((l) => l.text))}`).toBeDefined();
    const where = `${at}, ${kind}: "${line!.text}"`;
    expect(line!.level, `${where} says its severity in a word`).toMatch(/^(error|warning)$/);
    expect(line!.href, `${where} links to its section`).toBe(k.href);
    if (k.href) expect(line!.target, `${where}: ${k.href} exists`).toBe(true);
    expect(line!.box.top, `${where} starts on the first screen`).toBeGreaterThanOrEqual(0);
    expect(line!.box.bottom, `${where} ends on the first screen`).toBeLessThanOrEqual(v.height);
    expect(line!.box.left, where).toBeGreaterThanOrEqual(0);
    expect(line!.box.right, where).toBeLessThanOrEqual(v.width);
    expect(line!.covered, `${where} is not covered`).toBe(0);
  }
}

async function eachState(replay: Replay, check: (index: number) => Promise<void>): Promise<void> {
  await check(replay.index);
  while (replay.index + 1 < replay.states.length) {
    await replay.advance();
    await check(replay.index);
  }
}

test.describe('UXF-019', () => {
  const v = viewport('1280x720');
  const expected: Record<string, Kind[]> = { idle: [], 'token-flow': [], busy: ['route', 'watchdog', 'limit', 'pod'], errors: ['route', 'watchdog', 'limit', 'pod'] };
  for (const name of Object.keys(expected)) {
    for (const theme of THEMES) {
      test(`the alert strip names every current problem on the first screen at 1280x720, a link to its section each, and is absent when nothing is wrong — ${name} ${theme}`, async ({ browser }) => {
        const page = await pageAt(browser, v);
        try {
          const replay = await openReplay(page, scenario(name), theme);
          const seen = new Set<Kind>();
          await eachState(replay, async (i) => {
            const want = kindsOf(replay.states[i]!);
            for (const k of want) seen.add(k);
            await stripProblems(page, want, v, `${name} ${theme} state ${i}`);
          });
          expect([...seen].sort(), `${name}: the kinds its states hold`).toEqual([...expected[name]!].sort());
        } finally {
          await page.context().close();
        }
      });
    }
  }

  for (const theme of THEMES) {
    test(`a connection that is not live is a line of the strip, beside every other problem, on the first screen at 1280x720 — ${theme}`, async ({ browser }) => {
      const page = await pageAt(browser, v);
      try {
        const idle = await openReplay(page, scenario('idle'), theme);
        await stripProblems(page, [], v, `idle ${theme} live`);
        await dropConnection(idle.page);
        await stripProblems(page, ['connection'], v, `idle ${theme} not live`);
      } finally {
        await page.context().close();
      }
      const all = await pageAt(browser, v);
      try {
        const errors = await openReplay(all, scenario('errors'), theme);
        await dropConnection(errors.page);
        await stripProblems(all, ['connection', ...kindsOf(errors.states[errors.index]!)], v, `errors ${theme} not live`);
      } finally {
        await all.context().close();
      }
    });
  }

  test('a line of the strip opens its section below the sticky header', async ({ browser }) => {
    const page = await pageAt(browser, v);
    try {
      await openReplay(page, scenario('errors'), 'light');
      await page.getByRole('region', { name: 'Problems', exact: true }).getByRole('link', { name: /^Failed calls/ }).click({ timeout: 5_000 });
      await settle(page);
      const at = await page.evaluate(() => ({
        header: document.querySelector('header.top')!.getBoundingClientRect().bottom,
        heading: document.getElementById('h-routes')!.getBoundingClientRect().top,
        scrolled: window.scrollY,
      }));
      expect(at.scrolled, 'the page scrolled to Routes').toBeGreaterThan(0);
      expect(at.heading, 'the Routes heading is not under the header').toBeGreaterThanOrEqual(at.header);
      expect(at.heading, 'the Routes heading is on the screen').toBeLessThan(v.height);
    } finally {
      await page.context().close();
    }
  });
});
