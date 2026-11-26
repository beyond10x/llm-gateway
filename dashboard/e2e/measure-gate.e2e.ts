// Measures every scenario at the gating viewports in both themes. The assertions are in
// gates.e2e.ts, which runs after this file and reads what it writes.
import { expect, test } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { openReplay } from './harness.ts';
import { measure, writeMeasurement } from './measure.ts';
import { metricsDir } from './metrics-dir.ts';
import { canvasPixels } from './probe.ts';
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

// Break test: without reduced motion a canvas with traffic draws new pixels within 500 ms, so the
// reduced-motion check of gate 5 can fail. Skipped on a page without a canvas.
test('break: the canvas check sees a canvas that moves', async ({ page }) => {
  const scenario = SCENARIOS.find((s) => s.name === 'token-flow')!;
  const replay = await openReplay(page, scenario, 'light');
  test.skip((await page.locator('canvas').count()) === 0, 'the page has no canvas');
  while (replay.index + 1 < replay.states.length) await replay.advance();
  const before = await page.evaluate(canvasPixels);
  await page.waitForTimeout(500);
  expect(await page.evaluate(canvasPixels)).not.toEqual(before);
});
