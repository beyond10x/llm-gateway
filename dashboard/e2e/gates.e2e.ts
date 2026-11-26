// The hard gates (1, 2, 4 and 5 of the loop) on every scenario at the gating viewports in both
// themes, gate 2 also at the measured-only viewports, key data, the visible text (copy), the
// token-flow canvas in every scenario with active sessions, the replay's own soundness and its
// connection status, the theme toggle, and the ratchet against ux/baseline.json. Reads what
// measure-gate wrote; the theme toggle drives a page itself.
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { activeSessions } from '../src/flow.ts';
import { SCENARIOS, type Scenario } from '../ux/scenarios/index.ts';
import { aggregate, compare, describeProblem, measuredOnlyAggregate, mergeAggregates, type Baseline } from '../ux/ratchet.ts';
import { openReplay, themeToggleProblems } from './harness.ts';
import { stale, unexpected, wholeCheck, type Check } from './known-failures.ts';
import { headersDir, type HeaderMeasurement, type Measurement } from './measure.ts';
import { axeItems, expectedFetches, flowProblems, keyDataItems, screenOnlyProblems, statusProblems } from './metrics.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { GATING, MEASURED_ONLY, THEMES, type Theme } from './profiles.ts';

const BASELINE = fileURLToPath(new URL('../ux/baseline.json', import.meta.url));

function load(scenario: string, viewport: string, theme: string): Measurement {
  const file = join(metricsDir('gate'), `${scenario}__${viewport}__${theme}.json`);
  expect(existsSync(file), `no measurement ${file}`).toBe(true);
  return JSON.parse(readFileSync(file, 'utf8')) as Measurement;
}

function loadHeaders(scenario: string, theme: string): HeaderMeasurement {
  const file = join(headersDir(metricsDir('gate')), `${scenario}__${theme}.json`);
  expect(existsSync(file), `no header measurement ${file}`).toBe(true);
  return JSON.parse(readFileSync(file, 'utf8')) as HeaderMeasurement;
}

// A scenario whose last state shows an active session: its token-flow canvas must be drawn.
function withSessions(scenario: Scenario): boolean {
  const states = scenario.states(Date.now());
  return activeSessions(states[states.length - 1]!).size > 0;
}

type Itemised = Exclude<Check, 'flow'>;

// The items of one replay per itemised check; the known-failure keys.
function itemsOf(m: Measurement): Record<Itemised, string[]> {
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
    'key-data': keyDataItems(m),
    copy: m.details.copy.map((c) => c.key),
  };
}

const hiddenHeader = (items: { name: string; visible_ratio: number }[]) => items.filter((h) => h.visible_ratio < 1).map((h) => h.name);

for (const scenario of SCENARIOS) {
  const sessions = withSessions(scenario);
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      const at = `${scenario.name} ${viewport.name} ${theme}`;
      const items = () => itemsOf(load(scenario.name, viewport.name, theme));

      test(`gate 1: no horizontal scroll, nothing past its box, scrollers keyboard-reachable — ${at}`, () => {
        expect(unexpected('overflow', scenario.name, items().overflow, undefined, viewport.name)).toEqual([]);
      });

      test(`gate 1: truncated text carries its full value — ${at}`, () => {
        expect(unexpected('truncation', scenario.name, items().truncation, undefined, viewport.name)).toEqual([]);
      });

      test(`gate 2: the header is fully in view and uncovered at every state — ${at}`, () => {
        const m = load(scenario.name, viewport.name, theme);
        expect(m.details.header.map((h) => h.name)).toEqual(expect.arrayContaining(['h1', 'connection status']));
        if (m.toggle === 'present') expect(m.details.header.map((h) => h.name)).toContain('theme toggle');
        expect(unexpected('header', scenario.name, items().header, undefined, viewport.name)).toEqual([]);
      });

      test(`gate 5: axe-core reports no serious or critical violation — ${at}`, () => {
        expect(unexpected('axe', scenario.name, items().axe, undefined, viewport.name)).toEqual([]);
      });

      test(`gate 5: Tab reaches every control with a visible focus — ${at}`, () => {
        expect(unexpected('keyboard', scenario.name, items().keyboard, undefined, viewport.name)).toEqual([]);
      });

      test(`gate 5: reduced motion stops animations, transitions and the canvas — ${at}`, () => {
        expect(unexpected('reduced-motion', scenario.name, items()['reduced-motion'], undefined, viewport.name)).toEqual([]);
      });

      test(`key data: key text is found, key values and headings are 14 px or more and every other text 12 px or more at every state — ${at}`, () => {
        expect(unexpected('key-data', scenario.name, items()['key-data'], undefined, viewport.name)).toEqual([]);
      });

      test(`copy: no words run together, no raw identifier as a label, no control named by a glyph, at every state — ${at}`, () => {
        const m = load(scenario.name, viewport.name, theme);
        const label = new Map(m.details.copy.map((c) => [c.key, c.label]));
        expect(unexpected('copy', scenario.name, items().copy, undefined, viewport.name).map((k) => `${k} (${label.get(k)})`)).toEqual([]);
      });

      if (theme === 'light' && (scenario.truth?.failing_route || scenario.truth?.watchdog)) {
        test(`gate 4: the screen-only user reaches the truth of every task — ${scenario.name} ${viewport.name}`, () => {
          const m = load(scenario.name, viewport.name, theme);
          expect(m.screen_only.map((r) => r.task).sort()).toEqual([...(scenario.truth?.failing_route ? ['failing-route'] : []), ...(scenario.truth?.watchdog ? ['watchdog'] : [])].sort());
          expect(screenOnlyProblems(m.screen_only)).toEqual([]);
        });
      }

      if (sessions) {
        test(`flow: the token-flow canvas is drawn, also under reduced motion, and at least 95 % in view and uncovered — ${at}`, () => {
          const finding = wholeCheck('flow', scenario.name);
          test.fail(finding !== null, finding ?? undefined);
          expect(flowProblems(load(scenario.name, viewport.name, theme))).toEqual([]);
        });
      }
    }
  }

  for (const viewport of MEASURED_ONLY) {
    for (const theme of THEMES) {
      test(`gate 2: the header is fully in view and uncovered at every state — ${scenario.name} ${viewport.name} ${theme}`, () => {
        const items = loadHeaders(scenario.name, theme).viewports[viewport.name];
        expect(items, `${viewport.name} measured`).toBeDefined();
        expect(items!.map((h) => h.name)).toEqual(expect.arrayContaining(['h1', 'connection status', 'theme toggle']));
        expect(unexpected('header', scenario.name, hiddenHeader(items!), undefined, viewport.name)).toEqual([]);
      });
    }
  }

  test(`known failures: every item listed for the scenario still occurs — ${scenario.name}`, () => {
    const seen: Record<string, Set<string>> = {};
    const add = (check: string, list: string[]) => {
      for (const i of list) (seen[check] ??= new Set()).add(i);
    };
    for (const theme of THEMES) {
      for (const viewport of GATING) {
        for (const [check, list] of Object.entries(itemsOf(load(scenario.name, viewport.name, theme)))) add(check, list);
      }
      for (const items of Object.values(loadHeaders(scenario.name, theme).viewports)) add('header', hiddenHeader(items));
    }
    const checks: Itemised[] = ['overflow', 'truncation', 'header', 'axe', 'keyboard', 'reduced-motion', 'key-data', 'copy'];
    expect(checks.flatMap((c) => stale(c, scenario.name, seen[c] ?? new Set()))).toEqual([]);
  });

  const actions = scenario.actions ?? [];
  const lost = actions.includes('lose-connection');
  test(`replay: nothing left the browser, one snapshot fetch${lost ? ' and one failed poll' : ''}, and the stream read live at every state${lost ? ', and the status names the lost connection in words after it' : ''} — ${scenario.name}`, () => {
    for (const viewport of GATING) {
      for (const theme of THEMES) {
        const m = load(scenario.name, viewport.name, theme);
        const at = `${viewport.name} ${theme}`;
        expect.soft(m.blocked, `${at}: requests aborted`).toEqual([]);
        expect.soft(m.snapshot_fetches, `${at}: snapshot fetches`).toBe(expectedFetches(actions));
        expect.soft(statusProblems(m.status, m.states, actions, scenario.truth?.connection?.state), `${at}: connection status`).toEqual([]);
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
  const rows = SCENARIOS.flatMap((s) =>
    THEMES.flatMap((t) => Object.entries(loadHeaders(s.name, t).scrollers).map(([viewport, list]) => ({ scenario: s.name, viewport, scrollers: list.length }))),
  );
  const measured = mergeAggregates(aggregate(readMeasurements(metricsDir('gate'))), measuredOnlyAggregate(rows));
  expect(Object.keys(measured).sort()).toEqual(SCENARIOS.map((s) => s.name).sort());
  expect(compare(baseline, measured).map(describeProblem)).toEqual([]);
});
