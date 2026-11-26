// A round's measurement: every scenario at every viewport, gating and measured-only, in both
// themes, with a screenshot of the last state at the gating viewports. round-report.e2e.ts writes
// the round directory from it. Runs only through `pnpm ux:round <id>`.
import { join } from 'node:path';
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { measure, writeMeasurement } from './measure.ts';
import { metricsDir } from './metrics-dir.ts';
import { THEMES, VIEWPORTS } from './profiles.ts';

test.skip(!process.env.UX_ROUND, 'a round runs through `pnpm ux:round <id>`');

for (const scenario of SCENARIOS) {
  for (const viewport of VIEWPORTS) {
    for (const theme of THEMES) {
      test(`round ${scenario.name} ${viewport.name} ${theme}`, async ({ browser }) => {
        const dir = metricsDir('round');
        const shot = viewport.gating ? join(dir, 'shots', `${scenario.name}__${viewport.name}__${theme}.png`) : undefined;
        writeMeasurement(dir, await measure(browser, scenario, viewport, theme, shot));
      });
    }
  }
}
