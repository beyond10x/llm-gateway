// The regression assertions of change set R-04 (UXF-013, 006, 009, 012, 014), each run against the
// built page through the hermetic replay. UXF-015 is gate 2 at every viewport in gates.e2e.ts.
import { expect, test, type Browser, type Page } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import { INK, INK_GROUND, type Mark } from '../src/flow.ts';
import { Ledger, type SessionDef } from '../ux/scenarios/build.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, settle, type Replay } from './harness.ts';
import { THEMES, VIEWPORTS, type Viewport } from './profiles.ts';

const scenario = (name: string): Scenario => SCENARIOS.find((s) => s.name === name)!;

async function pageAt(browser: Browser, v: Viewport, opts: { reducedMotion?: boolean } = {}): Promise<Page> {
  const context = await browser.newContext({ viewport: { width: v.width, height: v.height }, timezoneId: 'UTC', locale: 'en-GB', reducedMotion: opts.reducedMotion ? 'reduce' : 'no-preference' });
  return context.newPage();
}

async function eachState(replay: Replay, check: (index: number) => Promise<void>): Promise<void> {
  await check(replay.index);
  while (replay.index + 1 < replay.states.length) {
    await replay.advance();
    await check(replay.index);
  }
}

// A session comes and goes and nothing else on the page changes: one call 9.5 minutes ago keeps
// every table as it is, and only the session's last-seen time moves inside the five minutes that
// make it active, then out again. "9m 30s ago" and "1m 30s ago" (a second less as the page's
// clock ticks) have the same width in tabular digits, so the Sessions table keeps its columns.
const A: SessionDef = { session: '4b7d20e9c1f3', label: 'claude-code' };
const comesAndGoes: Scenario = {
  name: 'session-comes-and-goes',
  description: 'No active session, then one, then none; no table changes.',
  anchor: false,
  states(base: number): Snapshot[] {
    const ledger = new Ledger();
    ledger.add({ session: A, route: 'claude-sonnet', at: base - 570_000, input: 12_400, output: 512 });
    const idle = ledger.snapshot(base, base - 3_600_000);
    const active: Snapshot = JSON.parse(JSON.stringify(idle));
    active.sessions[0]!.last_ms = base - 90_000;
    return [idle, active, JSON.parse(JSON.stringify(idle))];
  },
};

// The token-flow panel's height and the top of the section after it.
async function flowBox(page: Page): Promise<{ panel: number; next: number; sessions: string }> {
  return page.evaluate(() => {
    const panel = document.querySelector('.flow-panel');
    if (!panel) throw new Error('no token-flow panel');
    window.scrollTo(0, 0);
    const next = panel.nextElementSibling;
    return {
      panel: Math.round(panel.getBoundingClientRect().height * 100) / 100,
      next: Math.round((next?.getBoundingClientRect().top ?? 0) * 100) / 100,
      sessions: (panel.querySelector('canvas') ? (document.querySelector('.flow-empty') ? 'none' : 'some') : 'no canvas'),
    };
  });
}

test.describe('UXF-013', () => {
  for (const v of VIEWPORTS) {
    test(`the token flow panel keeps one height when sessions come and go, with no layout shift — ${v.name}`, async ({ browser }) => {
      const page = await pageAt(browser, v);
      try {
        const seen: string[] = [];
        const replay = await openReplay(page, comesAndGoes, 'light');
        await replay.takeShift();
        const first = await flowBox(page);
        expect(first.sessions, 'the first state has no active session').toBe('none');
        while (replay.index + 1 < replay.states.length) {
          await replay.takeShift();
          await replay.advance();
          const shift = await replay.takeShift();
          const box = await flowBox(page);
          seen.push(`state ${replay.index} (${box.sessions} active): panel ${box.panel} px, next section at ${box.next}, layout shift ${shift.toFixed(4)}`);
          expect.soft(box.panel, seen.at(-1)).toBe(first.panel);
          expect.soft(box.next, seen.at(-1)).toBe(first.next);
          expect.soft(shift, seen.at(-1)).toBe(0);
        }
        expect(seen[0], 'the second state has an active session').toMatch(/\(some active\)/);

        // The same height in the idle scenario and at every state of token-flow.
        for (const name of ['idle', 'token-flow']) {
          const other = await page.context().newPage();
          await eachState(await openReplay(other, scenario(name), 'light'), async (i) => {
            expect.soft((await flowBox(other)).panel, `${name} state ${i}`).toBe(first.panel);
          });
          await other.close();
        }
      } finally {
        await page.context().close();
      }
    });
  }
});

// Every word of a limit tile's value or second line that the browser broke over two lines; a word
// ends at white space or at an underscore, so a value may wrap at "_" but never inside a word.
function brokenWords(): string[] {
  const out: string[] = [];
  for (const el of document.querySelectorAll('.card .tile .value, .card .tile .sub')) {
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const text = n.textContent ?? '';
      for (const m of text.matchAll(/[^\s_]+/g)) {
        const range = document.createRange();
        range.setStart(n, m.index);
        range.setEnd(n, m.index + m[0].length);
        const lines = new Set([...range.getClientRects()].filter((r) => r.width > 0).map((r) => Math.round(r.top)));
        if (lines.size > 1) out.push(`"${m[0]}" in "${(el.textContent ?? '').trim()}"`);
      }
    }
  }
  return out;
}

test.describe('UXF-006', () => {
  for (const name of ['busy', 'errors']) {
    test(`limit status values never break inside a word, at every viewport and state, and keep the raw value in their title — ${name}`, async ({ page }) => {
      const replay = await openReplay(page, scenario(name), 'light');
      const found = new Set<string>();
      await eachState(replay, async () => {
        for (const v of VIEWPORTS) {
          await page.setViewportSize({ width: v.width, height: v.height });
          await settle(page);
          for (const w of await page.evaluate(brokenWords)) found.add(`${v.name}: ${w}`);
        }
      });
      expect([...found]).toEqual([]);
      // The Status tile of every limit names the raw header value in its title (none without one).
      const raw = replay.states[replay.states.length - 1]!.limits!.map((l) => {
        const h = Object.fromEntries(Object.entries(l.headers).map(([k, v]) => [k.toLowerCase(), v]));
        return h['anthropic-ratelimit-unified-status'] ?? null;
      });
      const titles = await page.evaluate(() => [...document.querySelectorAll('.card')].map((l) => l.querySelector('.tile .value')?.getAttribute('title') ?? null));
      expect(titles).toEqual(raw);
    });
  }
});

// The Watchdog heartbeat definition: its value text and whether it is marked as an alarm.
async function heartbeat(page: Page): Promise<{ value: string; alarm: boolean } | null> {
  return page.evaluate(() => {
    const dt = [...document.querySelectorAll('.stats dt')].find((d) => (d.textContent ?? '').trim() === 'Watchdog heartbeat');
    const dd = dt?.nextElementSibling;
    if (!dt || !dd) return null;
    return { value: (dd.textContent ?? '').replace(/\s+/g, ' ').trim(), alarm: dt.parentElement?.classList.contains('alarm') ?? false };
  });
}

// The last state of errors with a heartbeat one minute old.
const freshHeartbeat: Scenario = {
  name: 'fresh-heartbeat',
  description: 'errors with a watchdog heartbeat one minute old',
  anchor: false,
  states(base: number): Snapshot[] {
    const states = scenario('errors').states(base);
    const last: Snapshot = JSON.parse(JSON.stringify(states[states.length - 1]));
    last.provider!.watchdog_heartbeat_age_ms = 60_000;
    return [last];
  },
};

test.describe('UXF-009', () => {
  test('a stale watchdog heartbeat says "stale" in words beside its age; a fresh one does not', async ({ browser }) => {
    const page = await pageAt(browser, VIEWPORTS[0]!);
    try {
      for (const [name, age] of [['errors', '11m 0s ago'], ['busy', '7m 0s ago']] as const) {
        const p = await page.context().newPage();
        const replay = await openReplay(p, scenario(name), 'light');
        while (replay.index + 1 < replay.states.length) await replay.advance();
        const h = await heartbeat(p);
        expect(h, name).not.toBeNull();
        expect(h!.alarm, `${name}: marked as an alarm`).toBe(true);
        expect(h!.value, name).toContain(age);
        expect(h!.value, `${name}: says it in words`).toMatch(/\bstale\b/i);
        await p.close();
      }
      const fresh = await page.context().newPage();
      await openReplay(fresh, freshHeartbeat, 'light');
      const h = await heartbeat(fresh);
      expect(h).toEqual({ value: '1m 0s ago', alarm: false });
    } finally {
      await page.context().close();
    }
  });
});

// The caption of the token flow panel, with the text of its colour keys.
async function flowCaption(page: Page): Promise<{ text: string; keys: string[] }> {
  return page.evaluate(() => {
    const caption = document.querySelector('.flow-panel .flow-caption');
    if (!caption) throw new Error('no token-flow caption');
    return {
      text: (caption.textContent ?? '').replace(/\s+/g, ' ').trim(),
      keys: [...caption.querySelectorAll('.flow-key')].map((k) => (k.textContent ?? '').replace(/\s+/g, ' ').trim()),
    };
  });
}

test.describe('UXF-012', () => {
  test('the token flow caption says what the lines and dots mean and which way the dots move; under reduced motion it says motion is off and names no dot', async ({ browser }) => {
    for (const reducedMotion of [false, true]) {
      const page = await pageAt(browser, VIEWPORTS[0]!, { reducedMotion });
      try {
        const seen: string[] = [];
        const replay = await openReplay(page, scenario('token-flow'), 'light', { reducedMotion });
        await eachState(replay, async (index) => {
          const caption = await flowCaption(page);
          seen.push(caption.text);
          const at = `${reducedMotion ? 'reduced motion' : 'motion'}, state ${index}: "${caption.text}"`;
          expect(caption.text, `${at}: says a line joins a session to a route`).toMatch(/\bline\b.*\bsession\b.*\broute\b/i);
          expect(caption.text, `${at}: says how a call in flight looks`).toMatch(/\bin flight\b/i);
          if (reducedMotion) {
            expect(caption.text, `${at}: says motion is off`).toMatch(/\bmotion is off\b/i);
            expect(caption.keys, `${at}: names no dot it does not draw`).toEqual([]);
          } else {
            expect(caption.keys, at).toEqual(['input tokens', 'output tokens']);
            expect(caption.text, `${at}: says input dots move left to right`).toMatch(/input tokens\W+left to right/i);
            expect(caption.text, `${at}: says output dots move right to left`).toMatch(/output tokens\W+right to left/i);
            expect(caption.text, `${at}: says what a dot is`).toMatch(/\bdots?\b/i);
          }
        });
        expect(new Set(seen).size, 'the caption is the same in every state').toBe(1);
      } finally {
        await page.context().close();
      }
    }
  });
});

function relativeLuminance(rgb: readonly number[]): number {
  const [r, g, b] = rgb.map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  }) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function hex(value: string): number[] {
  const m = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(value.trim());
  if (!m) throw new Error(`not a #rgb or #rrggbb colour: '${value}'`);
  const digits = m[1]!.length === 3 ? [...m[1]!].map((d) => d + d).join('') : m[1]!;
  return [0, 2, 4].map((i) => parseInt(digits.slice(i, i + 2), 16));
}

// The contrast of a colour drawn at `alpha` over `ground`, against the ground.
function drawnContrast(colour: number[], alpha: number, ground: number[]): number {
  const over = colour.map((c, i) => c * alpha + ground[i]! * (1 - alpha));
  const [a, b] = [relativeLuminance(over), relativeLuminance(ground)].sort((x, y) => y - x) as [number, number];
  return (a + 0.05) / (b + 0.05);
}

test.describe('UXF-014', () => {
  test('every mark the token flow draws reaches 4.5:1 for text and 3:1 for strings, nodes and dots over the panel, in both themes', async ({ browser }) => {
    const page = await pageAt(browser, VIEWPORTS[0]!);
    try {
      const failures: string[] = [];
      for (const theme of THEMES) {
        await openReplay(page, scenario('token-flow'), theme);
        const names = [INK_GROUND, ...new Set(Object.values(INK).map((m) => m.colour))];
        const css = await page.evaluate((list) => {
          const style = getComputedStyle(document.documentElement);
          return Object.fromEntries(list.map((n) => [n, style.getPropertyValue(n).trim()]));
        }, names);
        const ground = hex(css[INK_GROUND]!);
        for (const mark of Object.keys(INK) as Mark[]) {
          const { colour, alpha, text } = INK[mark];
          const ratio = drawnContrast(hex(css[colour]!), alpha, ground);
          const floor = text ? 4.5 : 3;
          if (ratio < floor) failures.push(`${theme}: ${mark} (${colour} at ${alpha}) is ${ratio.toFixed(2)}:1, under ${floor}:1`);
        }
      }
      expect(failures).toEqual([]);
    } finally {
      await page.context().close();
    }
  });
});
