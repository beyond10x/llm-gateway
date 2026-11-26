// Break tests of the measuring tools: each feeds a probe or the harness a page that is wrong in
// one way and asserts the tool reports it, so a gate that passes is not passing by construction.
import { expect, test, type Page } from '@playwright/test';
import { SCENARIOS } from '../ux/scenarios/index.ts';
import { copyProbe } from './copy.ts';
import { fontMode, MONO_FAMILY } from './fonts.ts';
import { openReplay, themeToggleProblems } from './harness.ts';
import { reducedMotionReplay } from './measure.ts';
import { flowProblems, keyDataItems } from './metrics.ts';
import { canvasPixels, flowProbe, focusStep, headerProbe, keyDataProbe, layoutProbe } from './probe.ts';
import { playTask, type ScreenTask } from './screen-only.ts';

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

// The fonts Chromium drew each element's own text with, by CDP, and how wide the token-flow
// canvas's monospace stack is against the pinned mono face and the host's monospace.
async function renderedFonts(page: Page, selectors: string[]): Promise<{ text: Record<string, string[]>; canvas: { stack: number; pinned: number; host: number } }> {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('DOM.enable');
  await cdp.send('CSS.enable');
  const { root } = await cdp.send('DOM.getDocument', { depth: -1 });
  const text: Record<string, string[]> = {};
  for (const selector of selectors) {
    const { nodeId } = await cdp.send('DOM.querySelector', { nodeId: root.nodeId, selector });
    if (!nodeId) throw new Error(`no element ${selector}`);
    const { fonts } = await cdp.send('CSS.getPlatformFontsForNode', { nodeId });
    text[selector] = fonts.map((f) => `${f.familyName.replace(/ SmBld$/, '')}${f.isCustomFont ? '' : ' (host)'}`);
  }
  const canvas = await page.evaluate(({ stack, pinned }) => {
    const c = document.createElement('canvas').getContext('2d')!;
    const w = (font: string) => {
      c.font = `12px ${font}`;
      return c.measureText('0123456789 abcdef 12:34:56').width;
    };
    return { stack: w(stack), pinned: w(`'${pinned}'`), host: w('monospace') };
  }, { stack: "ui-monospace, 'SFMono-Regular', Menlo, Consolas, monospace", pinned: MONO_FAMILY });
  return { text, canvas };
}

test.describe('pinned fonts', () => {
  const SELECTORS = ['h1', 'section[aria-labelledby="h-calls"] thead th', 'section[aria-labelledby="h-calls"] tbody td.mono', 'section[aria-labelledby="h-calls"] tbody td:nth-child(3)'];
  const PINNED = { 'h1': ['IBM Plex Sans'], [SELECTORS[1]!]: ['IBM Plex Sans'], [SELECTORS[2]!]: ['IBM Plex Mono'], [SELECTORS[3]!]: ['IBM Plex Sans'] };

  test('every replay draws the page and its canvas with the committed fonts, not the host\'s', async ({ page }) => {
    test.skip(fontMode().kind !== 'pinned', 'UX_FONTS measures with host fonts');
    await openReplay(page, scenario('busy'), 'light');
    const { text, canvas } = await renderedFonts(page, SELECTORS);
    expect(text).toEqual(PINNED);
    expect(canvas.stack).toBe(canvas.pinned);
    expect(canvas.stack).not.toBe(canvas.host);
  });

  test('break: a replay with the host\'s fonts is told apart', async ({ page }) => {
    const before = process.env.UX_FONTS;
    process.env.UX_FONTS = 'host';
    try {
      await openReplay(page, scenario('busy'), 'light');
    } finally {
      if (before === undefined) delete process.env.UX_FONTS;
      else process.env.UX_FONTS = before;
    }
    const { text, canvas } = await renderedFonts(page, SELECTORS);
    expect(text).not.toEqual(PINNED);
    expect(Object.values(text).flat().every((f) => f.endsWith('(host)'))).toBe(true);
    expect(canvas.stack).toBe(canvas.host);
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

// A header with one always-visible control: a segmented control with rounded ends that clips its
// segments (as the page's theme toggle does), placed at `left` px, optionally under an overlay.
const headerHtml = (left: number, overlay: boolean) => `<!doctype html><html><body style="margin:0;background:#fff">
  <header style="position:relative;height:60px;background:#fff">
    <div data-ux-always="toggle" style="position:absolute;left:${left}px;top:12.5px;display:inline-flex;border:1px solid #80868f;border-radius:12px;overflow:hidden">
      <label style="display:block;padding:4px 12px;background:#2456c7;color:#fff">Light</label><label style="display:block;padding:4px 12px">Dark</label><label style="display:block;padding:4px 12px">System</label>
    </div>
    ${overlay ? '<div style="position:absolute;left:0;top:0;width:100%;height:60px;background:rgba(0,0,0,0.6)"></div>' : ''}
  </header></body></html>`;

test.describe('always-visible elements', () => {
  test('break: a rounded control fully in view and uncovered reads 1; one past the right edge or under an overlay reads less', async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const ratio = async (left: number, overlay = false) => {
      await page.setContent(headerHtml(left, overlay));
      return (await page.evaluate(headerProbe))[0]!.visible_ratio;
    };
    expect(await ratio(16)).toBe(1);
    expect(await ratio(16.4)).toBe(1);
    const past = await ratio(260);
    expect(past).toBeGreaterThan(0);
    expect(past).toBeLessThanOrEqual(0.8);
    expect(await ratio(16, true)).toBe(0);
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

// One page with every kind of copy defect once, and the forms the probe must leave alone: a space,
// a block boundary, generated content, a raw value in a code element or a `.mono` cell, a title
// attribute, a control with a word.
const COPY_PAGE = `<!doctype html><html><head><style>
  .b::before { content: ' · '; }
  .card h3 { display: flex; gap: 6px; }
</style></head><body><main>
<section aria-labelledby="h-a"><h2 id="h-a">Overview</h2>
  <div class="tile"><div class="label">Tokens in</div><div class="sub">1.88k tok/s<span class="muted">last 60 s</span></div></div>
  <div class="tile"><div class="label">Tokens out</div><div class="sub">45 tok/s <span class="muted">last 60 s</span></div></div>
  <div class="tile"><div class="label">five_hour</div><div class="value">five_<wbr>hour</div><div class="value" title="five_hour">Five hours</div></div>
  <div class="card"><h3>Subscription limits <span class="mono">3fa9c1d2</span></h3>
    <dl class="headers"><dt class="mono">anthropic-ratelimit-unified-status</dt><dd class="mono">allowed_warning</dd></dl></div>
  <p><span>alpha</span><span class="b">beta</span> <span style="display:none">hidden_word</span><code>five_hour</code></p>
  <div><div>gamma</div><div>delta</div></div>
</section>
<section aria-labelledby="h-b"><h2 id="h-b">Sessions</h2>
  <table><tbody><tr><th class="col-session">claude-code<span class="mono">3f9a1c0e</span></th><td class="col-calls mono">anthropic-ratelimit-requests-limit</td><td>200</td></tr></tbody></table>
  <button class="expand">▸ 2</button><button>▸ 2 attempts</button>
</section></main></body></html>`;

test.describe('visible text (copy)', () => {
  test('break: each kind of copy defect is reported once, at a place without its text, and the allowed forms are not', async ({ page }) => {
    await page.setContent(COPY_PAGE);
    const items = (await page.evaluate(copyProbe)).map((i) => i.split(' | ')[0]).sort();
    expect(items).toEqual([
      'glyph-control #h-b button.expand',
      'header-name #h-a dt.mono',
      'hex-title #h-a h3 span.mono',
      'run-together #h-a div.tile div.sub',
      'run-together #h-b th.col-session',
      'snake-label #h-a div.tile div.label',
      'snake-value #h-a div.tile div.value',
    ]);
  });

  test('break: on the page, the space taken out between the runs makes a run-together item', async ({ page }) => {
    await openReplay(page, scenario('busy'), 'light');
    const tiles = () => page.evaluate(copyProbe).then((items) => items.filter((i) => i.startsWith('run-together #h-kpi div.tile div.sub')));
    expect(await tiles(), 'the Tokens tiles set "last 60 s" apart from "tok/s"').toEqual([]);
    const removed = await page.evaluate(() => {
      let n = 0;
      document.querySelectorAll('.tile .sub .muted').forEach((m) => {
        const prev = m.previousSibling;
        if (prev?.nodeType === Node.TEXT_NODE && /\s$/.test(prev.textContent ?? '')) {
          prev.textContent = (prev.textContent ?? '').trimEnd();
          n += 1;
        }
      });
      return n;
    });
    expect(removed, 'a space before each window').toBeGreaterThan(0);
    expect((await tiles()).length).toBeGreaterThan(0);
  });
});

const FAILING: ScreenTask = { name: 'failing-route', follow: /error|fail/i, truth: [['claude-sonnet'], ['\\b429\\b']] };
const SPACER = (px: number) => `<div style="height:${px}px"></div>`;

async function play(page: Page, body: string, task: ScreenTask = FAILING) {
  await page.setViewportSize({ width: 800, height: 600 });
  await page.setContent(`<!doctype html><html><body style="margin:0;font:14px sans-serif">${body}</body></html>`);
  return playTask(page, task);
}

test.describe('screen-only user', () => {
  test('break: the truth at the top costs no action and glance 1; in the lower half glance 2; below the fold scrolls and glance 3', async ({ page }) => {
    expect(await play(page, `<p>claude-sonnet failed: 429 rate limited</p>${SPACER(3000)}`)).toMatchObject({ reached: true, actions: 0, looks: 1 });
    expect(await play(page, `${SPACER(400)}<p>claude-sonnet failed: 429</p>${SPACER(3000)}`)).toMatchObject({ reached: true, actions: 0, looks: 2 });
    expect(await play(page, `${SPACER(1500)}<p>claude-sonnet failed: 429</p>${SPACER(3000)}`)).toMatchObject({ reached: true, actions: 2, looks: 3, steps: ['look', 'scroll', 'look', 'scroll', 'look'] });
  });

  test('break: a visible link named for the task is one action; the same link covered is not used', async ({ page }) => {
    const body = (cover: boolean) =>
      `<p><a href="#calls">claude-sonnet: 4 errors</a></p>${cover ? '<div style="position:fixed;left:0;top:0;width:100%;height:80px;background:#fff"></div>' : ''}${SPACER(2000)}<section id="calls"><table><tr><td>claude-sonnet</td><td>429</td></tr></table></section>${SPACER(2000)}`;
    const followed = await play(page, body(false));
    expect(followed).toMatchObject({ reached: true, actions: 1, looks: 1 });
    expect(followed.steps).toContain('click link claude-sonnet: 4 errors');
    const covered = await play(page, body(true));
    expect(covered.steps.some((s) => s.startsWith('click'))).toBe(false);
    expect(covered).toMatchObject({ reached: true, looks: 3 });
  });

  test('break: a route and a cause in two different rows, or no truth at all, are not reached', async ({ page }) => {
    const split = await play(page, `<table><tbody><tr><td>claude-sonnet</td><td>200</td></tr><tr><td>qwen-coder</td><td>429</td></tr></tbody></table>${SPACER(1000)}`);
    expect(split).toMatchObject({ reached: false, looks: 3 });
    expect(split.steps[split.steps.length - 1]).toBe('end of page');
    expect(await play(page, `<p>all routes healthy</p>${SPACER(1000)}`)).toMatchObject({ reached: false });
  });
});

test.describe('key data', () => {
  test('break: a page with no key text is refused, a heading under 14 px and any other text under 12 px are reported', async ({ page }) => {
    await page.setContent('<main><p>only a paragraph</p></main>');
    const none = await page.evaluate(keyDataProbe);
    expect(none.key).toBeNull();
    expect(keyDataItems({ key_font_min_px: none.key?.px ?? null, details: { key_font_below: none.key_below, heading_font_below: none.heading_below, secondary_font_below: none.secondary_below } })).toContain('no key text');
    await page.setContent(`<main><h2 style="font-size:13.3px">Routes</h2><div class="card"><h3 style="font-size:12.6px">Limits <span class="mono">3fa9c1d2</span> <span class="small" style="font-size:11px">seen 5s ago</span></h3></div>
      <table><tbody><tr><td style="font-size:14px">qwen-coder</td></tr></tbody></table>
      <button style="font-size:11px">Show all 40</button><dl class="headers"><dt style="font-size:11.5px">status</dt><dd style="font-size:14px">allowed</dd></dl></main>`);
    const k = await page.evaluate(keyDataProbe);
    expect(k.key?.px).toBe(14);
    expect(k.heading_below).toEqual(['12.6px h3', '12.6px span.mono', '13.3px h2']);
    expect(k.secondary_below).toEqual(['11.5px dt', '11px button', '11px span.small']);
  });

  // The R-06 review's input: a tile value renamed `.metric-value` at 12 px read as 14 px key data.
  // Here every class is one a secondary text would have; the roles still say what is a value.
  const RENAMED = `<main>
    <section><div class="kpi"><div class="value">Tokens in</div><div class="metric-value small" style="font-size:12px">4.76M</div><div class="badge" style="font-size:14px">1.86k tok/s</div></div>
      <div class="kpi"><div class="sub"><abbr title="share of cached prompt tokens">Cache hit rate</abbr></div><div class="label" style="font-size:12px">88%</div></div></section>
    <table><tbody><tr><td class="label"><span class="small" style="font-size:12.5px">qwen-coder</span> <span class="badge" style="font-size:12.25px">vLLM</span></td><td style="font-size:14px">1.2k</td></tr>
      <tr><th class="badge" style="font-size:12px">claude-sonnet</th><td style="font-size:14px">3</td></tr></tbody></table>
    <dl class="stats"><dt style="font-size:12.25px">Name</dt><dd class="small" style="font-size:12px">runpod</dd></dl></main>`;

  test('break: a value at 12 px is key data by its role (a cell, a definition, what a metric shows), whatever its class', async ({ page }) => {
    await page.setContent(RENAMED);
    const labels = ['Tokens in', 'Cache hit rate', 'Name'];
    const k = await page.evaluate(keyDataProbe, labels);
    expect(k.key_below).toEqual(['12.5px span.small', '12px dd.small', '12px div.label', '12px div.metric-value.small', '12px th.badge']);
    expect(k.secondary_below, 'the labels and the badge after a value are secondary, at or over 12 px').toEqual([]);
    // Without the document's labels a metric's value is not known as one: the labels carry the role.
    expect((await page.evaluate(keyDataProbe, [])).key_below).toEqual(['12.5px span.small', '12px dd.small', '12px th.badge']);
  });
});

test.describe('truncation', () => {
  test('break: route cells cut with an ellipsis and no title are reported; with their full text in a title they are not', async ({ page }) => {
    await page.setViewportSize({ width: 1366, height: 768 });
    await openReplay(page, scenario('busy'), 'light');
    await page.addStyleTag({ content: 'td.col-route { max-width: 40px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }' });
    const cut = (await page.evaluate(layoutProbe)).truncated.filter((t) => t.includes('td.col-route'));
    expect(cut.length).toBeGreaterThan(0);
    await page.evaluate(() => document.querySelectorAll('td.col-route').forEach((td) => td.setAttribute('title', (td.textContent ?? '').trim())));
    expect((await page.evaluate(layoutProbe)).truncated.filter((t) => t.includes('td.col-route'))).toEqual([]);
  });
});

test.describe('connection loss', () => {
  test('break: the offline replay reads live until the stream errors, then offline in words, with one failed poll', async ({ page }) => {
    const replay = await openReplay(page, scenario('offline'), 'light');
    while (replay.index + 1 < replay.states.length) await replay.advance();
    expect(await replay.status()).toBe('live');
    await replay.act('lose-connection');
    expect(await replay.status()).toMatch(/offline/i);
    expect(replay.snapshotFetches).toBe(2);
    expect(replay.blocked).toEqual([]);
  });
});
