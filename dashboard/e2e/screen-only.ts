// The screen-only user of the loop (SKILL § 7): deterministic code, no model. It plays a task on the
// last state of a hermetic replay using only what a person sees: controls fully inside the viewport
// and not covered at their centre, found by accessible role and name. A click or a key press is one
// action, and so is a scroll; once it has scrolled, or an action moved the page by more than half a
// viewport (a link that jumps), every look costs glance 3.
//
// A task is reached when one element in view shows a match of every list of its truth (the
// scenario's `truth`): the smallest element whose readable text (truthInView) matches them all, no taller than
// half the viewport, fully in the viewport and uncovered at its centre, and not a container of
// several records (a table, a list, a section), so a route in one row and a cause in another do
// not count. Each turn it looks; when the
// truth is not in view it clicks the first visible control whose name points at the task (a link
// "claude-sonnet: 4 errors", a button "Watchdog"), else it scrolls down by 85 % of the viewport. It
// gives up at the end of the page or after MAX_ACTIONS.
//
// Glance: 1 when the truth is in the upper half of the viewport without a scroll (the primary area
// above the fold), 2 elsewhere in view without a scroll, 3 when it needed a scroll or a jump.
import type { Browser, Page } from '@playwright/test';
import type { Scenario, Truth } from '../ux/scenarios/index.ts';
import { openReplay, settle } from './harness.ts';
import type { Theme, Viewport } from './profiles.ts';

export const MAX_ACTIONS = 12;
export const SCROLL_SHARE = 0.85;

export interface ScreenTask {
  name: string;
  // The names of the controls that point at the task.
  follow: RegExp;
  // Regular expression sources, case-insensitive; one of each list must match.
  truth: string[][];
}

export interface TaskResult {
  task: string;
  reached: boolean;
  actions: number;
  // 1, 2 or 3; 3 when the task was not reached.
  looks: number;
  // What it did, in order: "look", "click <role> <name>", "scroll", "end of page".
  steps: string[];
  // The text of the element that showed the truth.
  seen: string;
}

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

// The tasks a scenario's truth defines: find the route with errors and its cause; read the
// watchdog state.
export function tasksOf(truth: Truth | undefined): ScreenTask[] {
  const out: ScreenTask[] = [];
  if (truth?.failing_route) out.push({ name: 'failing-route', follow: /error|fail/i, truth: [[escape(truth.failing_route.route)], truth.failing_route.cause] });
  if (truth?.watchdog) out.push({ name: 'watchdog', follow: /watchdog|heartbeat/i, truth: [['watchdog'], truth.watchdog.state] });
  return out;
}

export interface TruthView {
  found: boolean;
  // The truth's top in the viewport, as a share of its height.
  top: number;
  text: string;
}

// Runs in the page: the smallest element in view that shows the truth. Only text a person can read
// counts as shown: a text node whose box lies in the viewport and inside every ancestor that clips
// its overflow (text cut by an ellipsis is not read whole), with an opacity over 0.5 (its own and
// its ancestors' multiplied) and a colour contrast of at least 3:1 against the backgrounds behind it.
export function truthInView(truth: string[][]): TruthView {
  const res = truth.map((list) => list.map((s) => new RegExp(s, 'i')));
  const all = (text: string) => res.every((list) => list.some((re) => re.test(text)));
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  const norm = (s: string) => s.replace(/\s+/g, ' ').trim();

  type Rgba = [number, number, number, number];
  const paint = document.createElement('canvas').getContext('2d')!;
  const rgba = (c: string): Rgba => {
    paint.fillStyle = '#000';
    paint.fillStyle = c;
    const v = String(paint.fillStyle);
    if (v.startsWith('#')) return [parseInt(v.slice(1, 3), 16), parseInt(v.slice(3, 5), 16), parseInt(v.slice(5, 7), 16), 1];
    const p = (v.match(/[\d.]+/g) ?? []).map(Number);
    return [p[0] ?? 0, p[1] ?? 0, p[2] ?? 0, p[3] ?? 1];
  };
  const over = (top: Rgba, under: Rgba): Rgba => {
    const a = top[3];
    return [top[0] * a + under[0] * (1 - a), top[1] * a + under[1] * (1 - a), top[2] * a + under[2] * (1 - a), 1];
  };
  const lum = (c: Rgba): number => {
    const ch = (v: number) => {
      const s = v / 255;
      return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2]);
  };
  const contrast = (a: Rgba, b: Rgba): number => {
    const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x) as [number, number];
    return (hi + 0.05) / (lo + 0.05);
  };
  const readable = new Map<Element, boolean>();
  // Whether the text an element holds itself can be read: its opacity, its colour on its backdrop.
  const legible = (el: Element): boolean => {
    const known = readable.get(el);
    if (known !== undefined) return known;
    const chain: Element[] = [];
    for (let p: Element | null = el; p; p = p.parentElement) chain.unshift(p);
    let opacity = 1;
    let backdrop: Rgba = [255, 255, 255, 1];
    for (const p of chain) {
      const cs = getComputedStyle(p);
      opacity *= parseFloat(cs.opacity);
      const b = rgba(cs.backgroundColor);
      if (b[3] > 0) backdrop = over(b, backdrop);
    }
    const cs = getComputedStyle(el);
    const ink = rgba(cs.color);
    const shown = over([ink[0], ink[1], ink[2], ink[3] * opacity], backdrop);
    const ok = cs.visibility === 'visible' && opacity > 0.5 && contrast(shown, backdrop) >= 3;
    readable.set(el, ok);
    return ok;
  };
  // Whether a text box lies in the viewport and inside every ancestor that clips its overflow.
  const uncut = (box: DOMRect, el: Element): boolean => {
    if (box.top < -0.5 || box.left < -0.5 || box.bottom > vh + 0.5 || box.right > vw + 0.5) return false;
    for (let p: Element | null = el; p && p !== document.body && p !== document.documentElement; p = p.parentElement) {
      const cs = getComputedStyle(p);
      if (cs.overflowX === 'visible' && cs.overflowY === 'visible') continue;
      const r = p.getBoundingClientRect();
      if (box.left < r.left - 0.5 || box.right > r.right + 0.5 || box.top < r.top - 0.5 || box.bottom > r.bottom + 0.5) return false;
    }
    return true;
  };

  // The text a person can read in each element: its readable text nodes in document order, two
  // nodes with nothing but <wbr> or a comment between them read as one word.
  const seen = new Map<Element, string[]>();
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let last: Node | null = null;
  for (let n = walker.nextNode(); n; n = walker.nextNode()) {
    const text = n.textContent ?? '';
    const el = n.parentElement;
    if (!text.trim() || !el || el.closest('script, style, template, noscript')) continue;
    const range = document.createRange();
    range.selectNodeContents(n);
    const box = range.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0 || !uncut(box, el) || !legible(el)) continue;
    let before = n.previousSibling;
    while (before && (before.nodeName === 'WBR' || before.nodeType === Node.COMMENT_NODE || (before.nodeType === Node.TEXT_NODE && !before.textContent))) before = before.previousSibling;
    const piece = last !== null && before === last ? text : ` ${text}`;
    last = n;
    for (let p: Element | null = el; p && p !== document.documentElement; p = p.parentElement) {
      const list = seen.get(p);
      if (list) list.push(piece);
      else seen.set(p, [piece]);
    }
  }
  const touching = [...seen.keys()].filter((el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && r.bottom > 0 && r.top < vh && r.right > 0 && r.left < vw;
  });
  const texts = new Map(touching.map((el) => [el, norm(seen.get(el)!.join(''))]));
  const matching = touching.filter((el) => all(texts.get(el)!));
  const smallest = matching.filter((el) => !matching.some((other) => other !== el && el.contains(other)));
  const CONTAINER = 'table, thead, tbody, tfoot, ul, ol, section, main, article, nav, header, footer, form';
  for (const el of smallest) {
    if (el.matches(CONTAINER)) continue;
    const text = texts.get(el)!;
    const r = el.getBoundingClientRect();
    if (r.width <= 0 || r.height <= 0 || r.height > vh / 2) continue;
    if (r.top < -0.5 || r.left < -0.5 || r.bottom > vh + 0.5 || r.right > vw + 0.5) continue;
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    if (!hit || !(hit === el || el.contains(hit))) continue;
    return { found: true, top: r.top / vh, text: text.slice(0, 120) };
  }
  return { found: false, top: 0, text: '' };
}

// Runs in the page: whether an element is fully in the viewport and not covered at its centre.
export function inViewUncovered(el: Element): boolean {
  const r = el.getBoundingClientRect();
  const vw = document.documentElement.clientWidth;
  const vh = document.documentElement.clientHeight;
  if (r.width <= 0 || r.height <= 0) return false;
  if (r.top < -0.5 || r.left < -0.5 || r.bottom > vh + 0.5 || r.right > vw + 0.5) return false;
  const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
  return !!hit && (hit === el || el.contains(hit));
}

// Waits until the document stops scrolling (an anchor jump), at most 1 s.
async function steady(page: Page): Promise<void> {
  let last = -1;
  for (let i = 0; i < 20; i++) {
    await settle(page);
    const y = await page.evaluate(() => window.scrollY);
    if (y === last) return;
    last = y;
  }
}

// One task on the page as it is now (the replay's last state, scrolled to the top).
export async function playTask(page: Page, task: ScreenTask): Promise<TaskResult> {
  const steps: string[] = [];
  const used = new Set<string>();
  let actions = 0;
  let scrolled = false;
  await page.evaluate(() => window.scrollTo(0, 0));
  await steady(page);
  for (;;) {
    steps.push('look');
    const view = await page.evaluate(truthInView, task.truth);
    if (view.found) {
      const looks = scrolled ? 3 : view.top < 0.5 ? 1 : 2;
      return { task: task.name, reached: true, actions, looks, steps, seen: view.text };
    }
    if (actions >= MAX_ACTIONS) break;
    let clicked = false;
    for (const role of ['link', 'button'] as const) {
      const candidates = page.getByRole(role, { name: task.follow });
      const n = await candidates.count();
      for (let i = 0; i < n && !clicked; i++) {
        const c = candidates.nth(i);
        const name = ((await c.getAttribute('aria-label')) ?? (await c.innerText())).replace(/\s+/g, ' ').trim();
        const key = `${role} ${name}`;
        if (used.has(key) || !(await c.evaluate(inViewUncovered))) continue;
        used.add(key);
        const before = await page.evaluate(() => window.scrollY);
        await c.click();
        await page.mouse.move(0, 0);
        await steady(page);
        // A link that jumps the page moves the person's eyes like a scroll does.
        const jump = await page.evaluate((y) => Math.abs(window.scrollY - y) > window.innerHeight / 2, before);
        if (jump) scrolled = true;
        steps.push(`click ${key}${jump ? ' (the page jumped)' : ''}`);
        actions += 1;
        clicked = true;
      }
      if (clicked) break;
    }
    if (clicked) continue;
    const moved = await page.evaluate((share) => {
      const before = window.scrollY;
      window.scrollBy({ top: Math.round(window.innerHeight * share), behavior: 'instant' });
      return window.scrollY !== before;
    }, SCROLL_SHARE);
    if (!moved) {
      steps.push('end of page');
      break;
    }
    await settle(page);
    steps.push('scroll');
    actions += 1;
    scrolled = true;
  }
  return { task: task.name, reached: false, actions, looks: 3, steps, seen: '' };
}

// Every task of the scenario on a replay of it at the viewport, played after the last state, each
// from the top of the page. A task after one that pressed a button gets a fresh replay; a scroll or
// a link (an anchor jump) changes nothing a later task could read. Reduced motion, so an anchor
// jump does not animate.
export async function screenOnly(browser: Browser, scenario: Scenario, viewport: Viewport, theme: Theme): Promise<TaskResult[]> {
  const out: TaskResult[] = [];
  const context = await browser.newContext({ viewport: { width: viewport.width, height: viewport.height }, timezoneId: 'UTC', locale: 'en-GB', reducedMotion: 'reduce' });
  try {
    let page: Page | null = null;
    for (const task of tasksOf(scenario.truth)) {
      if (!page) {
        page = await context.newPage();
        const replay = await openReplay(page, scenario, theme, { reducedMotion: true });
        while (replay.index + 1 < replay.states.length) await replay.advance();
      }
      const result = await playTask(page, task);
      out.push(result);
      if (result.steps.some((s) => s.startsWith('click button'))) {
        await page.close();
        page = null;
      }
    }
  } finally {
    await context.close();
  }
  return out;
}
