// Measures every scenario at the gating viewports in both themes. The assertions are in
// gates.e2e.ts, which runs after this file and reads what it writes.
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { measure, writeMeasurement } from './measure.ts';
import { metricsDir } from './metrics-dir.ts';
import { GATING, THEMES } from './profiles.ts';

for (const scenario of SCENARIOS) {
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      test(`measure ${scenario.name} ${viewport.name} ${theme}`, async ({ browser }, info) => {
        const m = await measure(browser, scenario, viewport, theme);
        writeMeasurement(metricsDir(info, 'gate'), m);
      });
    }
  }
}
