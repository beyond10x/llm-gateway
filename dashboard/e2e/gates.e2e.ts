// The hard gates (1, 2 and 5 of the loop) on every scenario at the gating viewports in both
// themes, the token-flow canvas, the replay's own soundness, the theme toggle, and the ratchet
// against ux/baseline.json. Reads what measure-gate wrote; the theme toggle drives a page itself.
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { aggregate, compare, describeProblem, type Baseline } from '../ux/ratchet.ts';
import { openReplay, themeToggleProblems } from './harness.ts';
import { stale, unexpected, wholeCheck, type Check } from './known-failures.ts';
import type { Measurement } from './measure.ts';
import { axeItems } from './metrics.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { GATING, THEMES, type Theme } from './profiles.ts';

const BASELINE = fileURLToPath(new URL('../ux/baseline.json', import.meta.url));
const FLOW_VISIBLE_MIN = 0.95;

function load(scenario: string, viewport: string, theme: string): Measurement {
  const file = join(metricsDir('gate'), `${scenario}__${viewport}__${theme}.json`);
  expect(existsSync(file), `no measurement ${file}`).toBe(true);
  return JSON.parse(readFileSync(file, 'utf8')) as Measurement;
}

// The items of one replay per itemised check; the known-failure keys.
function itemsOf(m: Measurement): Record<Exclude<Check, 'flow'>, string[]> {
  return {
    overflow: [
      ...(m.metrics['overflow.x_px'] > 0 ? ['document scrolls sideways'] : []),
      ...m.details.overflowing.map((o) => o.key),
      ...m.details.scrollers_unreachable.map((s) => `no keyboard access: ${s}`),
    ],
    truncation: m.details.truncated.map((t) => t.key),
    header: m.details.header.filter((h) => h.visible_ratio < 1).map((h) => h.name),
    axe: axeItems(m.details.axe),
    keyboard: [...m.details.unreachable.map((u) => `unreachable: ${u}`), ...m.details.focus_invisible.map((f) => `focus invisible: ${f}`)],
    'reduced-motion': m.details.reduced_motion,
  };
}

for (const scenario of SCENARIOS) {
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      const at = `${scenario.name} ${viewport.name} ${theme}`;
      const items = () => itemsOf(load(scenario.name, viewport.name, theme));

      test(`gate 1: no horizontal scroll, nothing past its box, scrollers keyboard-reachable — ${at}`, () => {
        expect(unexpected('overflow', scenario.name, items().overflow)).toEqual([]);
      });

      test(`gate 1: truncated text carries its full value — ${at}`, () => {
        expect(unexpected('truncation', scenario.name, items().truncation)).toEqual([]);
      });

      test(`gate 2: the header is fully in view and uncovered at every state — ${at}`, () => {
        const m = load(scenario.name, viewport.name, theme);
        expect(m.details.header.map((h) => h.name)).toEqual(expect.arrayContaining(['h1', 'connection status']));
        if (m.toggle === 'present') expect(m.details.header.map((h) => h.name)).toContain('theme toggle');
        expect(unexpected('header', scenario.name, items().header)).toEqual([]);
      });

      test(`gate 5: axe-core reports no serious or critical violation — ${at}`, () => {
        expect(unexpected('axe', scenario.name, items().axe)).toEqual([]);
      });

      test(`gate 5: Tab reaches every control with a visible focus — ${at}`, () => {
        expect(unexpected('keyboard', scenario.name, items().keyboard)).toEqual([]);
      });

      test(`gate 5: reduced motion stops animations, transitions and the canvas — ${at}`, () => {
        expect(unexpected('reduced-motion', scenario.name, items()['reduced-motion'])).toEqual([]);
      });

      if (scenario.name === 'token-flow') {
        test(`flow: the token-flow canvas is drawn and at least 95 % in view and uncovered — ${at}`, () => {
          const finding = wholeCheck('flow', scenario.name);
          test.fail(finding !== null, finding ?? undefined);
          const m = load(scenario.name, viewport.name, theme);
          expect.soft(m.flow.present, 'a token-flow canvas').toBe(true);
          expect.soft(m.flow.drawn, `drawn (share ${m.flow.drawn_share})`).toBe(true);
          expect(m.flow.visible_ratio, 'share in view and uncovered').toBeGreaterThanOrEqual(FLOW_VISIBLE_MIN);
        });
      }
    }
  }

  test(`known failures: every item listed for the scenario still occurs — ${scenario.name}`, () => {
    const seen: Record<string, Set<string>> = {};
    for (const viewport of GATING) {
      for (const theme of THEMES) {
        for (const [check, list] of Object.entries(itemsOf(load(scenario.name, viewport.name, theme)))) {
          for (const i of list) (seen[check] ??= new Set()).add(i);
        }
      }
    }
    const checks: Exclude<Check, 'flow'>[] = ['overflow', 'truncation', 'header', 'axe', 'keyboard', 'reduced-motion'];
    expect(checks.flatMap((c) => stale(c, scenario.name, seen[c] ?? new Set()))).toEqual([]);
  });

  test(`replay: nothing left the browser, one snapshot fetch, and the stream read live at every state — ${scenario.name}`, () => {
    for (const viewport of GATING) {
      for (const theme of THEMES) {
        const m = load(scenario.name, viewport.name, theme);
        const at = `${viewport.name} ${theme}`;
        expect.soft(m.blocked, `${at}: requests aborted`).toEqual([]);
        expect.soft(m.snapshot_fetches, `${at}: snapshot fetches`).toBe(1);
        expect.soft(m.status.filter((s) => s !== 'live'), `${at}: status other than live`).toEqual([]);
        expect(m.theme_applied, `${at}: the emulated scheme shows its palette`).toBe(true);
      }
    }
  });
}

// Under the other system scheme, so the choice must change what the page shows.
for (const theme of THEMES) {
  const other: Theme = theme === 'dark' ? 'light' : 'dark';
  test(`affordance: a theme toggle in the header sets the theme — ${theme} under a ${other} system scheme, kept over a reload`, async ({ page }) => {
    const replay = await openReplay(page, SCENARIOS[0]!, other);
    expect(replay.toggle, 'a theme toggle').not.toBeNull();
    expect(await themeToggleProblems(replay, theme)).toEqual([]);
  });
}

test('ratchet: no metric is worse than ux/baseline.json, and every improvement is taken into it', () => {
  const baseline = JSON.parse(readFileSync(BASELINE, 'utf8')) as Baseline;
  const measured = aggregate(readMeasurements(metricsDir('gate')));
  expect(Object.keys(measured).sort()).toEqual(SCENARIOS.map((s) => s.name).sort());
  expect(compare(baseline, measured).map(describeProblem)).toEqual([]);
});
