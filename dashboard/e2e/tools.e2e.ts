// Break tests of the measuring tools: each feeds a probe or the harness a page that is wrong in
// one way and asserts the tool reports it, so a gate that passes is not passing by construction.
import { expect, test, type Page } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { openReplay, themeToggleProblems } from './harness.ts';
import { reducedMotionReplay } from './measure.ts';
import { flowProblems } from './metrics.ts';
import { canvasPixels, flowProbe, focusStep } from './probe.ts';

const scenario = (name: string) => SCENARIOS.find((s) => s.name === name)!;

test.describe('theme toggle check', () => {
  test('break: under the scheme it asks for, the check refuses to count the toggle', async ({ page }) => {
    const replay = await openReplay(page, scenario('idle'), 'dark');
    expect(await themeToggleProblems(replay, 'dark')).toContain('the page shows dark before the choice: emulate the other colour scheme first');
  });

  test('break: a toggle whose choice does nothing fails', async ({ page }) => {
    const replay = await openReplay(page, scenario('idle'), 'light');
    await page.evaluate(() => document.addEventListener('change', (e) => e.stopPropagation(), true));
    const problems = await themeToggleProblems(replay, 'dark');
    expect(problems).toContain('choosing dark shows light');
  });

  test('break: a choice that is not stored fails after the reload', async ({ page }) => {
    await page.addInitScript(() => {
      Storage.prototype.setItem = () => undefined;
    });
    const replay = await openReplay(page, scenario('idle'), 'light');
    expect(await themeToggleProblems(replay, 'dark')).toEqual(['after a reload the page shows light, not the stored dark']);
  });
});

// A canvas of `w` x `h` with a diagonal stroke, or blank.
const canvasHtml = (attrs: string, drawn: boolean) => `
  <canvas id="c" ${attrs}></canvas>
  <script>
    const c = document.getElementById('c');
    c.width = c.clientWidth; c.height = c.clientHeight;
    if (${drawn}) { const x = c.getContext('2d'); x.strokeStyle = '#2456c7'; x.lineWidth = 3; x.beginPath(); x.moveTo(0, 0); x.lineTo(c.width, c.height); x.stroke(); }
  </script>`;

async function flowOf(page: Page, body: string) {
  await page.setContent(`<!doctype html><html><body style="margin:0;background:#f7f7f8">${body}</body></html>`);
  return page.evaluate(flowProbe);
}

test.describe('token-flow canvas metric', () => {
  test('break: a drawn canvas fixed behind opaque panels is drawn but not visible', async ({ page }) => {
    const flow = await flowOf(
      page,
      `${canvasHtml('class="flow" style="position:fixed;inset:0;width:100%;height:100%;pointer-events:none;z-index:0"', true)}
       <main style="position:relative;z-index:1;padding:16px"><section style="background:#fff;height:1600px">panels</section></main>`,
    );
    expect(flow.present).toBe(true);
    expect(flow.drawn).toBe(true);
    expect(flow.visible_ratio).toBeLessThan(0.2);
  });

  test('break: a blank canvas in its own panel is visible but not drawn', async ({ page }) => {
    const flow = await flowOf(page, `<section><h2>Token flow</h2>${canvasHtml('aria-hidden="true" style="display:block;width:600px;height:180px"', false)}</section>`);
    expect(flow.visible_ratio).toBeGreaterThanOrEqual(0.95);
    expect(flow.drawn).toBe(false);
  });

  test('break: a drawn panel below the fold is not visible', async ({ page }) => {
    const flow = await flowOf(page, `<div style="height:2000px"></div><section><h2>Token flow</h2>${canvasHtml('aria-hidden="true" style="display:block;width:600px;height:180px"', true)}</section>`);
    expect(flow.drawn).toBe(true);
    expect(flow.visible_ratio).toBe(0);
  });

  test('a drawn canvas in its own panel at the top passes', async ({ page }) => {
    const flow = await flowOf(
      page,
      `<section><h2>Token flow</h2>${canvasHtml('aria-hidden="true" style="display:block;width:600px;height:180px;pointer-events:none"', true)}</section><section style="background:#fff;height:1600px">panels</section>`,
    );
    expect(flow).toMatchObject({ present: true, drawn: true });
    expect(flow.visible_ratio).toBeGreaterThanOrEqual(0.95);
  });

  test('break: a token-flow canvas that draws nothing under reduced motion fails the flow gate', async ({ browser }) => {
    const run = async (blank: boolean) => {
      const context = await browser.newContext({ viewport: { width: 1366, height: 768 }, reducedMotion: 'reduce' });
      try {
        const page = await context.newPage();
        if (blank) {
          await page.addInitScript(() => {
            for (const k of ['stroke', 'fill', 'fillText'] as const) CanvasRenderingContext2D.prototype[k] = () => undefined;
          });
        }
        return await reducedMotionReplay(page, scenario('token-flow'), 'light');
      } finally {
        await context.close();
      }
    };
    const [drawn, blank] = await Promise.all([run(false), run(true)]);
    expect(drawn.flow.drawn, 'the page draws a static canvas under reduced motion').toBe(true);
    expect(drawn.violations).toEqual([]);
    expect(blank.flow).toMatchObject({ present: true, drawn: false });
    const shown = { present: true, at: 'canvas', visible_ratio: 1, drawn: true, drawn_share: 0.05 };
    expect(flowProblems({ flow: shown, reduced_flow: blank.flow })).toEqual(['the canvas is blank under reduced motion (share drawn 0)']);
  });

  test('break: without reduced motion the canvas check sees a canvas that moves', async ({ page }) => {
    const replay = await openReplay(page, scenario('token-flow'), 'dark');
    test.skip((await page.locator('canvas').count()) === 0, 'the page has no canvas');
    while (replay.index + 1 < replay.states.length) await replay.advance();
    const before = await page.evaluate(canvasPixels);
    await page.waitForTimeout(500);
    expect(await page.evaluate(canvasPixels)).not.toEqual(before);
  });
});

test.describe('visible focus', () => {
  const PAGE = `<!doctype html><html><body style="background:#ffffff;margin:20px">
    <style>
      button { outline: none; margin: 8px; }
      #none:focus { outline: none; }
      #resting { box-shadow: 0 0 0 0 transparent; }
      #resting:focus { outline: none; }
      #faint:focus { outline: 2px solid #f0f0f0; }
      #clear:focus { outline: 2px solid rgba(36, 86, 199, 0); }
      #ghost { opacity: 0; }
      #ghost:focus { outline: 2px solid #2456c7; }
      #ring:focus { outline: 2px solid #2456c7; outline-offset: 2px; }
      #shadow:focus { box-shadow: 0 0 0 3px #1b1d21; }
      .radio { position: relative; display: inline-block; }
      .radio input { position: absolute; inset: 0; margin: 0; opacity: 0; }
      .radio span { display: block; padding: 2px 8px; }
      .radio input:focus + span { outline: 2px solid #2456c7; outline-offset: -4px; }
    </style>
    <button id="none">none</button><button id="resting">resting shadow</button><button id="faint">faint</button>
    <button id="clear">clear</button><button id="ghost">ghost</button><button id="ring">ring</button>
    <button id="shadow">shadow</button>
    <label class="radio"><input id="radio" type="radio" name="t"><span>Light</span></label>
  </body></html>`;

  test('break: no ring, a resting transparent shadow, a faint ring, a clear ring and an invisible control are invisible; a contrasting ring is visible', async ({ page }) => {
    await page.setContent(PAGE);
    const visible: Record<string, boolean> = {};
    for (const id of ['none', 'resting', 'faint', 'clear', 'ghost', 'ring', 'shadow', 'radio']) {
      await page.focus(`#${id}`);
      visible[id] = (await page.evaluate(focusStep)).visible;
    }
    expect(visible).toEqual({ none: false, resting: false, faint: false, clear: false, ghost: false, ring: true, shadow: true, radio: true });
  });
});

test.describe('replay', () => {
  test('break: the per-state layout shift does not depend on a click just before the states', async ({ browser }) => {
    const run = async (clickFirst: boolean) => {
      const context = await browser.newContext({ viewport: { width: 1280, height: 720 }, timezoneId: 'UTC', locale: 'en-GB' });
      try {
        const page = await context.newPage();
        const replay = await openReplay(page, scenario('token-flow'), 'light');
        if (clickFirst) await replay.click(page.getByRole('radio', { name: /^System/ }));
        const shifts: number[] = [];
        while (replay.index + 1 < replay.states.length) {
          await replay.takeShift();
          await replay.advance();
          shifts.push(await replay.takeShift());
        }
        return shifts;
      } finally {
        await context.close();
      }
    };
    const [plain, clicked] = await Promise.all([run(false), run(true)]);
    expect(Math.max(...plain)).toBeGreaterThan(0.02);
    expect(clicked.length).toBe(plain.length);
    clicked.forEach((v, i) => expect(Math.abs(v - plain[i]!), `state ${i + 1}: ${v} against ${plain[i]}`).toBeLessThan(0.005));
  });

  test('break: the replay reads live on the stream, and a failed stream reads otherwise', async ({ page }) => {
    const replay = await openReplay(page, scenario('token-flow'), 'light');
    const seen = [await replay.status()];
    while (replay.index + 1 < replay.states.length) {
      await replay.advance();
      seen.push(await replay.status());
    }
    expect(seen.every((s) => s === 'live'), seen.join(', ')).toBe(true);
    expect(replay.snapshotFetches).toBe(1);
    expect(replay.blocked).toEqual([]);
    await page.evaluate(() => (window as unknown as { __uxStream: { fail(): void } }).__uxStream.fail());
    await expect.poll(() => replay.status()).not.toBe('live');
  });
});
