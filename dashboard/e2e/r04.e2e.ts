// The regression assertions of change set R-04 (UXF-013, 006, 009, 012, 014), each run against the
// built page through the hermetic replay. UXF-015 is gate 2 at every viewport in gates.e2e.ts.
import { expect, test, type Browser, type Page } from '@playwright/test';
import type { Snapshot } from '../src/extra.ts';
import { Ledger, type SessionDef } from '../ux/scenarios/build.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { openReplay, type Replay } from './harness.ts';
import { VIEWPORTS, type Viewport } from './profiles.ts';

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

// A session comes and goes and nothing else on the page changes: one call nine minutes ago keeps
// every table as it is, and only the session's last-seen time moves inside the five minutes that
// make it active, then out again. "9m 0s ago" and "1m 0s ago" are the same width in tabular digits.
const A: SessionDef = { session: '4b7d20e9c1f3', label: 'claude-code' };
const comesAndGoes: Scenario = {
  name: 'session-comes-and-goes',
  description: 'No active session, then one, then none; no table changes.',
  anchor: false,
  states(base: number): Snapshot[] {
    const ledger = new Ledger();
    ledger.add({ session: A, route: 'claude-sonnet', at: base - 9 * 60_000, input: 12_400, output: 512 });
    const idle = ledger.snapshot(base, base - 3_600_000);
    const active: Snapshot = JSON.parse(JSON.stringify(idle));
    active.sessions[0]!.last_ms = base - 60_000;
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
