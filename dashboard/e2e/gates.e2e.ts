// The hard gates (1, 2 and 5 of the loop) on every scenario at the gating viewports in both
// themes, the theme toggle, and the ratchet against ux/baseline.json. Reads what measure-gate wrote.
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, test, type TestInfo } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { aggregate, compare, describeProblem, type Baseline } from '../ux/ratchet.ts';
import { knownFailure, type Gate } from './known-failures.ts';
import type { Measurement } from './measure.ts';
import { metricsDir, readMeasurements } from './metrics-dir.ts';
import { GATING, THEMES } from './profiles.ts';

const BASELINE = fileURLToPath(new URL('../ux/baseline.json', import.meta.url));

function load(info: TestInfo, scenario: string, viewport: string, theme: string): Measurement {
  const file = join(metricsDir(info, 'gate'), `${scenario}__${viewport}__${theme}.json`);
  expect(existsSync(file), `no measurement ${file}`).toBe(true);
  return JSON.parse(readFileSync(file, 'utf8')) as Measurement;
}

function gate(name: Gate, scenario: string): void {
  const finding = knownFailure(name, scenario);
  test.fail(finding !== null, finding ?? undefined);
}

for (const scenario of SCENARIOS) {
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      const at = `${scenario.name} ${viewport.name} ${theme}`;

      test(`gate 1: no horizontal scroll, nothing past its box, scrollers keyboard-reachable — ${at}`, async ({}, info) => {
        gate('gate1-overflow', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect.soft(m.metrics['overflow.x_px'], 'horizontal document scroll').toBe(0);
        expect.soft(m.details.overflowing, 'elements past their box').toEqual([]);
        expect(m.details.scrollers_unreachable, 'scroll containers without keyboard access').toEqual([]);
      });

      test(`gate 1: truncated text carries its full value — ${at}`, async ({}, info) => {
        gate('gate1-truncation', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect(m.details.truncated).toEqual([]);
      });

      test(`gate 2: the header is fully in view and uncovered at every state — ${at}`, async ({}, info) => {
        gate('gate2-always-visible', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect(m.details.header.map((h) => h.name)).toEqual(expect.arrayContaining(['h1', 'connection status']));
        if (m.toggle === 'present') expect(m.details.header.map((h) => h.name)).toContain('theme toggle');
        for (const h of m.details.header) expect(h.visible_ratio, h.name).toBe(1);
      });

      test(`gate 5: axe-core reports no serious or critical violation — ${at}`, async ({}, info) => {
        gate('gate5-axe', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect(m.details.axe).toEqual([]);
      });

      test(`gate 5: Tab reaches every control with a visible focus — ${at}`, async ({}, info) => {
        gate('gate5-keyboard', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect.soft(m.details.unreachable, 'controls Tab never reaches').toEqual([]);
        expect(m.details.focus_invisible, 'controls focused without outline or box-shadow').toEqual([]);
      });

      test(`gate 5: reduced motion stops animations, transitions and the canvas — ${at}`, async ({}, info) => {
        gate('gate5-reduced-motion', scenario.name);
        const m = load(info, scenario.name, viewport.name, theme);
        expect(m.details.reduced_motion).toEqual([]);
      });
    }
  }

  test(`affordance: a theme toggle in the header sets the theme — ${scenario.name}`, async ({}, info) => {
    gate('theme-toggle', scenario.name);
    for (const viewport of GATING) {
      for (const theme of THEMES) {
        const m = load(info, scenario.name, viewport.name, theme);
        expect(m.toggle, `${viewport.name} ${theme}`).toBe('present');
        expect(m.theme_applied, `${viewport.name} ${theme}`).toBe(true);
      }
    }
  });
}

test('ratchet: no metric is worse than ux/baseline.json, and every improvement is taken into it', async ({}, info) => {
  const baseline = JSON.parse(readFileSync(BASELINE, 'utf8')) as Baseline;
  const measured = aggregate(readMeasurements(metricsDir(info, 'gate')));
  expect(Object.keys(measured).sort()).toEqual(SCENARIOS.map((s) => s.name).sort());
  expect(compare(baseline, measured).map(describeProblem)).toEqual([]);
});
