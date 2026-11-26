// Measures every scenario at the gating viewports in both themes, and the always-visible elements
// at the measured-only viewports. The assertions are in gates.e2e.ts, which runs after this file
// and reads what it writes.
import { test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { measure, measureHeaders, writeHeaderMeasurement, writeMeasurement } from './measure.ts';
import { metricsDir } from './metrics-dir.ts';
import { GATING, MEASURED_ONLY, THEMES } from './profiles.ts';

for (const scenario of SCENARIOS) {
  for (const viewport of GATING) {
    for (const theme of THEMES) {
      test(`measure ${scenario.name} ${viewport.name} ${theme}`, async ({ browser }) => {
        writeMeasurement(metricsDir('gate'), await measure(browser, scenario, viewport, theme));
      });
    }
  }
  for (const theme of THEMES) {
    test(`measure the header of ${scenario.name} at ${MEASURED_ONLY.map((v) => v.name).join(', ')} ${theme}`, async ({ browser }) => {
      writeHeaderMeasurement(metricsDir('gate'), await measureHeaders(browser, scenario, theme, MEASURED_ONLY));
    });
  }
}
